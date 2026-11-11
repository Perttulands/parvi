//! The Terminals page in the app against a real node (terminals over a
//! scratch tmux, started through the testkit): a session attached to a
//! window takes what is typed and shows its output, and a toy page hands a
//! reference into that session through the Send ask and starts sessions
//! through the Launch ask; other pages read the sessions the page publishes
//! and open views beside it. Linux and macOS only;
//! skips without tmux unless `PARVI_REQUIRE_TMUX=1`.
#![cfg(unix)]

use std::cell::RefCell;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, Window, div, prelude::*, px};
use parvi_app_kit::ask::SessionTarget;
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::{Answers, Ask, Host, Mounted, Page, mount};
use parvi_app_shell::Shell;
use parvi_core::NodeId;
use parvi_node_kit::Listing;
use parvi_terminals_app::Terminals;
use parvi_terminals_app::store::sessions::Key;
use parvi_terminals_contract::SessionList;
use parvi_testkit::node::Node;
use parvi_testkit::tmux::Server;

const AREAS: &[Listing] = &[parvi_terminals::LISTING];

thread_local! {
    static PAGE: RefCell<Option<Entity<Terminals>>> = const { RefCell::new(None) };
    static ITEM_ASK: RefCell<Option<Ask>> = const { RefCell::new(None) };
}

/// The Terminals page, kept where the test can read what it shows.
fn terminals(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
    mount(host, window, cx, |host, window, cx| {
        PAGE.with(|p| *p.borrow_mut() = Some(cx.entity()));
        Terminals::new(host, window, cx)
    })
}

/// A page that hands a reference to a session, as any page may.
struct Toy {
    host: Host,
}

/// What the toy hands over: a command, so the session's output shows it
/// arrived and ran.
const REFERENCE: &str = "echo sent-$((40+2))";

impl Page for Toy {
    const ID: &'static str = "toy";
    const TITLE: &'static str = "Toy";
    const ANSWERS: &'static [Answers] = &[Answers::Reveal("item")];

    fn answer(&mut self, ask: Ask, from: &'static str, _: &mut Window, cx: &mut Context<Self>) {
        if ask.beside() {
            self.host.show_beside(from, cx.new(|_| Note).into(), cx);
        }
        ITEM_ASK.with(|slot| *slot.borrow_mut() = Some(ask));
    }
}

impl Render for Toy {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let host = self.host.clone();
        div()
            .size_full()
            .child(
                div()
                    .id("toy-send")
                    .debug_selector(|| "toy-send".into())
                    .w(px(80.))
                    .h(px(30.))
                    .child("Send")
                    .on_click({
                        let host = host.clone();
                        move |_, _, cx| {
                            let ask = Ask::Send {
                                reference: REFERENCE.into(),
                                target: None,
                            };
                            host.ask(ask, cx).expect("the Terminals page answers it");
                        }
                    }),
            )
            .child(
                div()
                    .id("toy-send-beta")
                    .debug_selector(|| "toy-send-beta".into())
                    .w(px(80.))
                    .h(px(30.))
                    .child("Send to beta")
                    .on_click(move |_, _, cx| {
                        let node = host.nodes(cx).remove(0).id;
                        let target = SessionTarget {
                            node,
                            user: String::new(),
                            name: "beta".into(),
                        };
                        let ask = Ask::Send {
                            reference: REFERENCE.into(),
                            target: Some(target),
                        };
                        host.ask(ask, cx).expect("the Terminals page answers it");
                    }),
            )
    }
}

fn toy(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
    mount(host, window, cx, |host, _, _| Toy { host })
}

/// A running node, on its own thread and runtime.
struct Running {
    url: String,
    node: String,
    socket: std::path::PathBuf,
    fixture: std::path::PathBuf,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A node following a scratch tmux server with one session, `alpha`.
fn start(label: &str) -> Option<Running> {
    start_with_harness(label, false)
}

fn start_with_harness(label: &str, harness: bool) -> Option<Running> {
    start_with_restart_harness(label, harness, false)
}

fn start_with_restart_harness(label: &str, harness: bool, restart: bool) -> Option<Running> {
    let label = label.to_string();
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("a runtime");
        rt.block_on(async move {
            let Some(tmux) = Server::start_in(&label) else {
                let _ = ready.send(None);
                return;
            };
            tmux.new_session("alpha", 100, 30);
            tmux.new_session("beta", 100, 30);
            let dir = parvi_testkit::fresh_dir("flags-ui");
            let binary = dir.join("fake");
            let launch = dir.join("launch.conf");
            if harness {
                parvi_testkit::tmux::write_script(
                    &binary,
                    format!("#!/bin/sh\nif [ \"$1\" = --help ]; then printf 'Options:\\n  -v, --verbose  Verbose output\\n'; exit; fi\nprintf '%s|%s\\n' \"$PWD\" \"$*\" >> '{}'\nwhile :; do sleep 1; done\n", dir.join("runs").display()),
                );
                let config = if restart {
                    format!("harness fake {} --marker\nfolder {}\n", binary.display(), dir.display())
                } else {
                    format!("harness fake {}\n", binary.display())
                };
                std::fs::write(&launch, config).unwrap();
            }
            let config = format!(
                "[terminals]\nsocket = {:?}\nlaunch = {:?}\n",
                tmux.socket_path(),
                launch
            );
            let node = Node::start(AREAS, &config).await.expect("a node");
            let _ = ready.send(Some((node.url(), node.id(), tmux.socket_path(), dir.clone())));
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            node.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    });
    match wait.recv().expect("the node thread") {
        Some((url, node, socket, fixture)) => Some(Running {
            url,
            node,
            socket,
            fixture,
            stop,
            thread: Some(thread),
        }),
        None => {
            let _ = thread.join();
            assert!(
                std::env::var_os("PARVI_REQUIRE_TMUX").is_none(),
                "PARVI_REQUIRE_TMUX is set but tmux did not start"
            );
            None
        }
    }
}

fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut ok: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        if ok(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn click(cx: &mut VisualTestContext, selector: String) {
    let selector: &'static str = Box::leak(selector.into_boxed_str());
    let at = cx.debug_bounds(selector).expect(selector).center();
    cx.simulate_click(at, Modifiers::none());
}

/// What the page's first window shows.
fn shown(cx: &mut VisualTestContext) -> String {
    let page = PAGE.with(|p| p.borrow().clone()).expect("the page");
    page.read_with(cx, |p, _| p.window_text(0))
        .unwrap_or_default()
}

/// The app with the Terminals page and the toy against `running`, the
/// page shown and `alpha` attached to its first window.
fn open_with_alpha<'a>(
    running: &Running,
    name: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext, std::path::PathBuf) {
    let settings = std::env::temp_dir()
        .join(format!("parvi-terminals-{name}-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().expect("its folder")).expect("a folder");
    // The node and the link run on their own threads.
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (url, path) = (running.url.clone(), settings.clone());
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[terminals, toy], Some(url), Some(path), window, cx)
    });
    until(cx, "the link to open", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    until(cx, "the Terminals page to show", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("terminals")
    });
    // The rail lists alpha; a click peeks, and Attach puts it in a window.
    let row = format!("row-{}::alpha", running.node);
    let leaked: &'static str = Box::leak(row.clone().into_boxed_str());
    until(cx, "alpha in the rail", |cx| {
        cx.debug_bounds(leaked).is_some()
    });
    click(cx, row);
    until(cx, "the peek", |cx| {
        cx.debug_bounds("peek-attach").is_some()
    });
    click(cx, "peek-attach".into());
    until(cx, "alpha's screen in the window", |cx| {
        !shown(cx).trim().is_empty()
    });
    (shell, cx, settings)
}

#[gpui::test]
fn a_session_in_a_window_takes_typing_and_shows_its_output(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-page-typing") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "typing", cx);
    cx.simulate_input("echo typed-$((6*7))");
    cx.simulate_keystrokes("enter");
    until(cx, "the output in the window", |cx| {
        shown(cx).contains("typed-42")
    });
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}

#[gpui::test]
fn a_page_hands_a_reference_into_a_session_through_send(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-page-send") else {
        return;
    };
    let (shell, cx, settings) = open_with_alpha(&running, "send", cx);
    click(cx, "tab-toy".into());
    until(cx, "the toy page to show", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("toy")
    });
    click(cx, "toy-send".into());
    // The ask brings the Terminals page forward with Send aimed at the
    // focused window's session; Send pastes the reference and submits it.
    until(cx, "the Send drawer", |cx| {
        cx.debug_bounds("send-go").is_some()
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.shown_page()),
        Some("terminals")
    );
    click(cx, "send-go".into());
    until(cx, "the reference's output in the session", |cx| {
        shown(cx).contains("sent-42")
    });
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}

#[gpui::test]
fn an_ask_that_names_a_session_aims_send_at_it_not_at_the_focused_one(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-page-target") else {
        return;
    };
    let (shell, cx, settings) = open_with_alpha(&running, "target", cx);
    click(cx, "tab-toy".into());
    until(cx, "the toy page to show", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("toy")
    });
    click(cx, "toy-send-beta".into());
    until(cx, "the Send drawer", |cx| {
        cx.debug_bounds("send-go").is_some()
    });
    let page = PAGE.with(|p| p.borrow().clone()).expect("the page");
    let aimed = page.read_with(cx, |p, _| p.send_target.clone());
    assert_eq!(
        aimed,
        Some(Key {
            node: running.node.clone(),
            user: String::new(),
            name: "beta".into()
        }),
        "alpha has the focus; the ask names beta"
    );
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}

struct Note;

impl Render for Note {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child("beside")
    }
}

#[gpui::test]
fn other_pages_read_the_sessions_and_open_views_beside_them(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-page-shared") else {
        return;
    };
    let (shell, cx, settings) = open_with_alpha(&running, "shared", cx);
    let toy = shell.read_with(cx, |s, _| Host::new(s.hub()).for_page("toy", &[]));
    let sessions = cx
        .update(|_, cx| toy.model::<SessionList>(cx))
        .expect("the hub");
    let listed = sessions.read_with(cx, |l, _| {
        l.sessions
            .iter()
            .map(|s| format!("{}/{}", s.node, s.name))
            .collect::<Vec<_>>()
    });
    assert!(
        listed.contains(&format!("{}/alpha", running.node)),
        "{listed:?}"
    );
    assert!(cx.debug_bounds("terminals-beside").is_none());
    cx.update(|_, cx| toy.show_beside("terminals", cx.new(|_| Note).into(), cx));
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("terminals-beside").is_some(),
        "drawn beside"
    );
    cx.update(|_, cx| toy.close_beside("terminals", cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("terminals-beside").is_none(), "closed");
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}

#[gpui::test]
fn a_page_starts_a_session_in_a_folder_through_launch(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-page-launch") else {
        return;
    };
    let (shell, cx, settings) = open_with_alpha(&running, "launch", cx);
    let toy = shell.read_with(cx, |s, _| Host::new(s.hub()).for_page("toy", &[]));
    let sessions = cx
        .update(|_, cx| toy.model::<SessionList>(cx))
        .expect("the hub");
    let folder = settings.parent().expect("its folder").to_string_lossy();
    // The suggested name is taken, so the session takes the next free one.
    let ask = Ask::Launch {
        node: NodeId(running.node.clone()),
        folder: folder.to_string(),
        name: "alpha".into(),
    };
    cx.update(|_, cx| toy.ask(ask, cx))
        .expect("the Terminals page answers it");
    until(cx, "alpha-2 to be listed in the folder", |cx| {
        sessions.read_with(cx, |l, _| {
            l.sessions.iter().any(|s| {
                s.node == running.node
                    && s.name == "alpha-2"
                    && s.cwd.as_deref().is_some_and(|c| c.ends_with(&*folder))
            })
        })
    });
    // A node the app does not know: the status line says why.
    let ask = Ask::Launch {
        node: NodeId("nowhere".into()),
        folder: "/".into(),
        name: "lost".into(),
    };
    cx.update(|_, cx| toy.ask(ask, cx))
        .expect("the Terminals page answers it");
    cx.run_until_parked();
    let said = shell.read_with(cx, |s, cx| {
        let status = s.hub().read(cx).status.clone();
        status.map(|s| (s.text, s.failure))
    });
    assert_eq!(
        said,
        Some((
            "Could not start lost: its node is not in the fleet".into(),
            true
        ))
    );
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}

fn tmux(running: &Running, args: &[&str]) -> String {
    let out =
        std::process::Command::new(std::env::var_os("PARVI_TMUX").unwrap_or_else(|| "tmux".into()))
            .args(["-S", running.socket.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn scroll_to_launch(cx: &mut VisualTestContext, scroll: &str) {
    let selector: &'static str = Box::leak(scroll.to_string().into_boxed_str());
    let point = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_event(gpui::ScrollEvent {
        position: point,
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-1000.))),
        modifiers: Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
        source: gpui::ScrollSource::Pointer,
    });
    cx.run_until_parked();
}

#[gpui::test]
fn launches_use_each_tile_grid(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-sized-launches") else {
        return;
    };
    let settings = parvi_testkit::fresh_dir("sized-settings").join("app.json");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let path = settings.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[terminals, toy], Some(url), Some(path), window, cx)
    });
    cx.simulate_resize(gpui::size(px(1440.), px(900.)));
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    cx.update(|_, cx| {
        page.update(cx, |p, cx| {
            p.settings.workspace.set_windows(0, 4);
            cx.notify();
        })
    });
    until(cx, "four tile grids", |cx| {
        page.read_with(cx, |p, _| {
            p.launch_grids.borrow().iter().all(Option::is_some)
        })
    });
    for i in 0..4 {
        let grid = page.read_with(cx, |p, _| p.launch_grids.borrow()[i].unwrap());
        let button = format!("launch-{i}");
        let selector: &'static str = Box::leak(button.clone().into_boxed_str());
        until(cx, &format!("launcher ready in tile {i}"), |cx| {
            cx.debug_bounds(selector).is_some()
        });
        scroll_to_launch(cx, &format!("launcher-scroll-{i}"));
        let viewport: &'static str = Box::leak(format!("launcher-scroll-{i}").into_boxed_str());
        until(cx, &format!("visible launch button in tile {i}"), |cx| {
            cx.debug_bounds(selector)
                .zip(cx.debug_bounds(viewport))
                .is_some_and(|(button, viewport)| viewport.contains(&button.center()))
        });
        click(cx, button);
        until(cx, &format!("session bound in tile {i}"), |cx| {
            page.read_with(cx, |p, _| p.settings.workspace.shown(i).is_some())
        });
        let name = page.read_with(cx, |p, _| {
            p.settings.workspace.shown(i).unwrap().key.name.clone()
        });
        assert_eq!(
            tmux(
                &running,
                &[
                    "display-message",
                    "-p",
                    "-t",
                    &name,
                    "#{window_width}x#{window_height}"
                ]
            ),
            format!("{}x{}", grid.0, grid.1)
        );
        until(cx, "terminal painted", |cx| {
            page.read_with(cx, |p, _| p.geo.borrow()[i].body.is_some())
        });
        let font = cx.update(|window, cx| {
            let p = page.read(cx);
            let geo = p.geo.borrow()[i];
            parvi_terminals_app::term::paint::fit(
                window,
                geo.body.unwrap().size,
                grid.0 as usize,
                grid.1 as usize,
                p.settings.font_size,
            )
            .font_size
        });
        assert!(f32::from(font) >= 11.);
    }
    let grid = page.read_with(cx, |p, _| {
        p.launch_grids.borrow()[p.settings.workspace.focus].unwrap()
    });
    let host = shell.read_with(cx, |s, _| Host::new(s.hub()).for_page("toy", &[]));
    let folder = settings.parent().unwrap().to_string_lossy().to_string();
    cx.update(|_, cx| {
        host.ask(
            Ask::Launch {
                node: NodeId(running.node.clone()),
                folder,
                name: "asked-size".into(),
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "asked session", |_cx| {
        tmux(&running, &["list-sessions", "-F", "#{session_name}"])
            .lines()
            .any(|s| s == "asked-size")
    });
    assert_eq!(
        tmux(
            &running,
            &[
                "display-message",
                "-p",
                "-t",
                "asked-size",
                "#{window_width}x#{window_height}"
            ]
        ),
        format!("{}x{}", grid.0, grid.1)
    );
    let before = tmux(&running, &["list-sessions", "-F", "#{session_name}"]);
    cx.update(|_, cx| {
        host.ask(
            Ask::Send {
                reference: "hello".into(),
                target: None,
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "new agent choice", |cx| {
        cx.debug_bounds("target-new").is_some()
    });
    click(cx, "target-new".into());
    until(cx, "send launcher", |cx| {
        cx.debug_bounds("send-body").is_some()
    });
    scroll_to_launch(cx, "send-body");
    until(cx, "visible send launch button", |cx| {
        cx.debug_bounds("launch-4")
            .zip(cx.debug_bounds("send-body"))
            .is_some_and(|(button, viewport)| viewport.contains(&button.center()))
    });
    click(cx, "launch-4".into());
    let mut new_name = String::new();
    until(cx, "send session", |_cx| {
        let now = tmux(&running, &["list-sessions", "-F", "#{session_name}"]);
        if let Some(name) = now
            .lines()
            .find(|name| !before.lines().any(|old| old == *name))
        {
            new_name = name.to_string();
            true
        } else {
            false
        }
    });
    assert_eq!(
        tmux(
            &running,
            &[
                "display-message",
                "-p",
                "-t",
                &new_name,
                "#{window_width}x#{window_height}"
            ]
        ),
        format!("{}x{}", grid.0, grid.1)
    );
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn launcher_browses_and_selects_node_flags(cx: &mut TestAppContext) {
    let Some(running) = start_with_harness("terminals-flags", true) else {
        return;
    };
    let settings = parvi_testkit::fresh_dir("flags-ui-settings").join("app.json");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let path = settings.clone();
    let (_shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[terminals], Some(url), Some(path), window, cx)
    });
    until(cx, "fake harness", |cx| {
        cx.debug_bounds("harness-0-fake").is_some()
    });
    click(cx, "harness-0-fake".into());
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    until(cx, "fake harness selected", |cx| {
        page.read_with(cx, |p, cx| {
            p.launchers[0].read(cx).harness.as_deref() == Some("fake")
        })
    });
    until(cx, "visible Browse control", |cx| {
        cx.debug_bounds("flags-browse-0")
            .zip(cx.debug_bounds("launcher-scroll-0"))
            .is_some_and(|(button, viewport)| viewport.contains(&button.center()))
    });
    click(cx, "flags-browse-0".into());
    cx.run_until_parked();
    scroll_to_launch(cx, "launcher-scroll-0");
    until(cx, "verbose flag", |cx| {
        cx.debug_bounds("flag-0---verbose").is_some()
    });
    click(cx, "flag-0---verbose".into());
    until(cx, "flag in preview", |cx| {
        cx.debug_bounds("flag-line-0---verbose").is_some()
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn restart_recovers_the_launched_harness_and_folder(cx: &mut TestAppContext) {
    let Some(running) = start_with_restart_harness("terminals-restart-harness", true, true) else {
        return;
    };
    let settings = parvi_testkit::fresh_dir("restart-harness-settings").join("app.json");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let path = settings.clone();
    let (_shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[terminals], Some(url), Some(path), window, cx)
    });
    until(cx, "fake harness", |cx| {
        cx.debug_bounds("harness-0-fake").is_some()
    });
    click(cx, "harness-0-fake".into());
    scroll_to_launch(cx, "launcher-scroll-0");
    until(cx, "launch button", |cx| {
        cx.debug_bounds("launch-0").is_some()
    });
    click(cx, "launch-0".into());
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    until(cx, "session bound", |cx| {
        page.read_with(cx, |p, _| p.settings.workspace.shown(0).is_some())
    });
    let name = page.read_with(cx, |p, _| {
        p.settings.workspace.shown(0).unwrap().key.name.clone()
    });
    // The node starts its shell through a launch gate and types the harness
    // into it: tmux's start command therefore identifies the gate, while the
    // fake's run records below prove which harness and flags actually ran.
    assert!(
        tmux(
            &running,
            &[
                "display-message",
                "-p",
                "-t",
                &name,
                "#{pane_start_command}"
            ]
        )
        .contains("wait-for")
    );
    let runs = running.fixture.join("runs");
    until(cx, "fake harness started", |_| {
        std::fs::read_to_string(&runs).is_ok_and(|s| s.lines().count() == 1)
    });
    assert_eq!(
        tmux(
            &running,
            &["display-message", "-p", "-t", &name, "#{pane_current_path}"]
        ),
        running.fixture.display().to_string()
    );
    tmux(&running, &["kill-session", "-t", &name]);
    until(cx, "ended tile", |cx| {
        cx.debug_bounds("ended-0-0").is_some()
    });
    click(cx, "ended-0-0".into());
    until(cx, "fake harness restarted", |_| {
        std::fs::read_to_string(&runs).is_ok_and(|s| s.lines().count() == 2)
    });
    assert_eq!(
        tmux(
            &running,
            &["display-message", "-p", "-t", &name, "#{pane_current_path}"]
        ),
        running.fixture.display().to_string()
    );
    assert!(
        tmux(
            &running,
            &[
                "display-message",
                "-p",
                "-t",
                &name,
                "#{pane_start_command}"
            ]
        )
        .contains("wait-for")
    );
    let records = std::fs::read_to_string(runs).unwrap();
    assert!(
        records
            .lines()
            .all(|line| line == format!("{}|--marker", running.fixture.display()))
    );
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn restart_of_external_session_uses_last_folder_and_says_shell(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-restart-external") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "restart-external", cx);
    let folder = settings.parent().unwrap().to_path_buf();
    tmux(
        &running,
        &[
            "send-keys",
            "-t",
            "alpha",
            &format!("cd '{}'", folder.display()),
            "Enter",
        ],
    );
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    let key = Key {
        node: running.node.clone(),
        user: String::new(),
        name: "alpha".into(),
    };
    until(cx, "last folder observed", |cx| {
        page.read_with(cx, |p, _| {
            p.settings
                .restarts
                .get(&key.text())
                .is_some_and(|r| r.folder == folder.to_string_lossy())
        })
    });
    tmux(&running, &["kill-session", "-t", "alpha"]);
    until(cx, "ended tile", |cx| {
        cx.debug_bounds("ended-0-0").is_some()
    });
    let note = format!(
        "ended-note-0-alpha has ended. Restart opens a shell in {}.",
        folder.display()
    );
    let selector: &'static str = Box::leak(note.into_boxed_str());
    assert!(cx.debug_bounds(selector).is_some());
    click(cx, "ended-0-0".into());
    until(cx, "alpha restarted", |_| {
        tmux(&running, &["list-sessions", "-F", "#{session_name}"])
            .lines()
            .any(|name| name == "alpha")
    });
    assert_eq!(
        tmux(
            &running,
            &[
                "display-message",
                "-p",
                "-t",
                "alpha",
                "#{pane_current_path}"
            ]
        ),
        folder.display().to_string()
    );
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn send_chooses_a_pane_and_defaults_to_the_focused_pane(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-send-panes") else {
        return;
    };
    let (shell, cx, settings) = open_with_alpha(&running, "send-panes", cx);
    tmux(
        &running,
        &["split-window", "-d", "-t", "alpha:0", "env PS1='$ ' sh"],
    );
    let ids = tmux(
        &running,
        &["list-panes", "-t", "alpha:0", "-F", "#{pane_id}"],
    );
    let panes: Vec<String> = ids.lines().map(str::to_owned).collect();
    assert_eq!(panes.len(), 2);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    let key = Key {
        node: running.node.clone(),
        user: String::new(),
        name: "alpha".into(),
    };
    until(cx, "two panes listed", |cx| {
        page.read_with(cx, |p, _| {
            p.fleet.session(&key).is_some_and(|s| s.panes.len() == 2)
        })
    });
    let host = shell.read_with(cx, |s, _| Host::new(s.hub()).for_page("toy", &[]));
    let target = SessionTarget {
        node: NodeId(running.node.clone()),
        user: String::new(),
        name: "alpha".into(),
    };
    cx.update(|_, cx| {
        host.ask(
            Ask::Send {
                reference: "echo pane-second-$((40+2))".into(),
                target: Some(target.clone()),
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "pane chooser", |cx| {
        cx.debug_bounds("send-pane-1").is_some()
    });
    assert!(cx.debug_bounds("send-pane-selected-0").is_some());
    click(cx, "send-pane-1".into());
    until(cx, "second pane selected", |cx| {
        cx.debug_bounds("send-pane-selected-1").is_some()
    });
    click(cx, "send-go".into());
    until(cx, "text in second pane", |_| {
        tmux(&running, &["capture-pane", "-p", "-t", &panes[1]]).contains("pane-second-42")
    });
    assert!(!tmux(&running, &["capture-pane", "-p", "-t", &panes[0]]).contains("pane-second-42"));
    until(cx, "drawer closed", |cx| {
        cx.debug_bounds("send-go").is_none()
    });
    let second = panes[1].trim_start_matches('%').parse::<u32>().unwrap();
    cx.update(|_, cx| {
        page.update(cx, |p, cx| {
            p.settings.workspace.choose_pane(0, second);
            cx.notify();
        })
    });
    cx.update(|_, cx| {
        host.ask(
            Ask::Send {
                reference: "echo next".into(),
                target: Some(target.clone()),
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "focused pane selected by default", |cx| {
        cx.debug_bounds("send-pane-selected-1").is_some()
    });
    // The target can also be shown in a different tile; its displayed pane
    // remains the default even when another tile has focus.
    cx.update(|_, cx| {
        page.update(cx, |p, cx| {
            p.settings.workspace.set_windows(0, 2);
            p.settings
                .workspace
                .bind_in(0, 1, key.clone(), Some(second));
            p.settings.workspace.focus = 0;
            cx.notify();
        })
    });
    cx.update(|_, cx| {
        host.ask(
            Ask::Send {
                reference: "echo next".into(),
                target: Some(target),
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "other tile's pane selected", |cx| {
        cx.debug_bounds("send-pane-selected-1").is_some()
    });
    // Explicitly select that pane, then let it disappear: the drawer must
    // report the loss instead of silently sending into the other pane.
    click(cx, "send-pane-selected-1".into());
    tmux(&running, &["kill-pane", "-t", &panes[1]]);
    until(cx, "remaining pane offered", |cx| {
        cx.debug_bounds("send-pane-0").is_some()
    });
    click(cx, "send-go".into());
    until(cx, "pane loss reported", |cx| {
        page.read_with(cx, |p, _| {
            p.send
                .said
                .as_deref()
                .is_some_and(|s| s.contains("selected pane") && s.contains("gone"))
        })
    });
    assert!(!tmux(&running, &["capture-pane", "-p", "-t", &panes[0]]).contains("echo next"));
    click(cx, "send-pane-0".into());
    let first = panes[0].trim_start_matches('%').parse::<u32>().unwrap();
    until(cx, "remaining pane selected", |cx| {
        page.read_with(cx, |p, _| p.send.pane == Some(first))
    });
    cx.update(|_, cx| {
        host.ask(
            Ask::Send {
                reference: "echo single".into(),
                target: Some(SessionTarget {
                    node: NodeId(running.node.clone()),
                    user: String::new(),
                    name: "beta".into(),
                }),
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "single-pane target", |cx| {
        page.read_with(cx, |p, _| {
            p.send_target.as_ref().is_some_and(|k| k.name == "beta")
        })
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("send-pane-0").is_none());
    assert!(cx.debug_bounds("send-pane-selected-0").is_none());
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_send_accepts_an_explicit_pane_id() {
    let Some(server) = Server::start_in("terminals-cli-send-pane") else {
        return;
    };
    server.new_session("cli-alpha", 100, 30);
    server.run(&["split-window", "-d", "-t", "cli-alpha:0", "env PS1='$ ' sh"]);
    let ids = server.run(&["list-panes", "-t", "cli-alpha:0", "-F", "#{pane_id}"]);
    let panes: Vec<&str> = ids.lines().collect();
    assert_eq!(panes, ["%0", "%1"]);
    let config = format!("[terminals]\nsocket = {:?}\n", server.socket_path());
    let node = Node::start(AREAS, &config).await.unwrap();
    parvi_testkit::tmux::eventually("both panes listed", || {
        node.sessions()
            .list()
            .iter()
            .find(|s| s.name == "cli-alpha" && s.panes.len() == 2)
            .map(|_| ())
    })
    .await;
    let url = node.url();
    let args = [
        "send",
        "cli-alpha",
        "--pane",
        "1",
        "--text",
        "echo cli-pane-42",
        "--node",
        &url,
    ]
    .map(str::to_owned);
    assert_eq!(
        parvi_node_kit::cli::run("session", &args, &mut Vec::new())
            .await
            .unwrap(),
        0
    );
    parvi_testkit::tmux::eventually("CLI text in pane 1", || {
        server
            .run(&["capture-pane", "-p", "-t", "%1"])
            .contains("cli-pane-42")
            .then_some(())
    })
    .await;
    assert!(
        !server
            .run(&["capture-pane", "-p", "-t", "%0"])
            .contains("cli-pane-42")
    );
    node.stop().await;
}

#[gpui::test]
fn reveal_session_shows_the_requested_session_and_pane(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-reveal") else {
        return;
    };
    let (shell, cx, settings) = open_with_alpha(&running, "reveal", cx);
    tmux(
        &running,
        &["split-window", "-d", "-t", "beta:0", "env PS1='$ ' sh"],
    );
    let ids = tmux(
        &running,
        &["list-panes", "-t", "beta:0", "-F", "#{pane_id}"],
    );
    let panes: Vec<u32> = ids
        .lines()
        .map(|s| s.trim_start_matches('%').parse().unwrap())
        .collect();
    assert_eq!(panes.len(), 2);
    assert_eq!(panes[0], 1, "the test's first beta pane is %1");
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    let beta = Key {
        node: running.node.clone(),
        user: String::new(),
        name: "beta".into(),
    };
    until(cx, "beta's two panes", |cx| {
        page.read_with(cx, |p, _| {
            p.fleet.session(&beta).is_some_and(|s| s.panes.len() == 2)
        })
    });
    let beta_id = page.read_with(cx, |p, _| p.fleet.session(&beta).unwrap().id.clone());
    click(cx, "tab-toy".into());
    until(cx, "toy page", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("toy")
    });
    let host = shell.read_with(cx, |s, _| Host::new(s.hub()).for_page("toy", &[]));
    cx.update(|_, cx| {
        host.ask(
            Ask::Reveal {
                what: "session".into(),
                beside: false,
                id: "beta".into(),
                node: None,
                part: None,
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "beta revealed without a pane", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("terminals")
            && page.read_with(cx, |p, _| {
                p.settings.workspace.shown(0).is_some_and(|b| b.key == beta)
            })
    });
    cx.update(|_, cx| {
        host.ask(
            Ask::Reveal {
                what: "session".into(),
                beside: false,
                id: "beta".into(),
                node: None,
                part: Some("1".into()),
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "beta on the terminals page", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("terminals")
            && page.read_with(cx, |p, _| {
                p.settings
                    .workspace
                    .shown(0)
                    .is_some_and(|b| b.key == beta && b.pane == Some(1))
            })
    });
    until(cx, "beta pane painted", |cx| {
        page.read_with(cx, |p, _| p.window_text(0).is_some())
    });
    assert!(cx.update(|window, cx| page.read(cx).term_focus.is_focused(window)));
    cx.simulate_input("echo revealed-pane-$((40+2))");
    cx.simulate_keystrokes("enter");
    until(cx, "typing reaches pane 1", |_| {
        tmux(&running, &["capture-pane", "-p", "-t", "%1"]).contains("revealed-pane-42")
    });
    assert!(
        !tmux(
            &running,
            &["capture-pane", "-p", "-t", &format!("%{}", panes[1])]
        )
        .contains("revealed-pane-42")
    );

    // A bound session in a hidden tile is restored there, not moved into
    // the focused tile; an id and a %pane work as well as a name and number.
    cx.update(|_, cx| {
        page.update(cx, |p, cx| {
            p.settings.workspace.bind_in(0, 1, beta.clone(), Some(1));
            p.settings.workspace.set_windows(0, 1);
            p.settings.workspace.focus = 0;
            cx.notify();
        })
    });
    cx.update(|_, cx| {
        host.ask(
            Ask::Reveal {
                what: "session".into(),
                beside: false,
                id: beta_id,
                node: Some(NodeId(running.node.clone())),
                part: Some(format!("%{}", panes[1])),
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "hidden bound tile restored", |cx| {
        page.read_with(cx, |p, _| {
            p.settings.workspace.focus == 1
                && p.settings
                    .workspace
                    .shown(1)
                    .is_some_and(|b| b.key == beta && b.pane == Some(panes[1]))
        })
    });
    let before = page.read_with(cx, |p, _| p.settings.workspace.clone());
    cx.update(|_, cx| {
        host.ask(
            Ask::Reveal {
                what: "session".into(),
                beside: false,
                id: "missing".into(),
                node: Some(NodeId(running.node.clone())),
                part: None,
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "missing session reported", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).status.as_ref().is_some_and(|status| {
                status.failure && status.text == format!("no session missing on {}", running.node)
            })
        })
    });
    assert_eq!(
        page.read_with(cx, |p, _| p.settings.workspace.clone()),
        before
    );
    cx.update(|_, cx| {
        host.ask(
            Ask::Reveal {
                what: "session".into(),
                beside: false,
                id: "beta".into(),
                node: Some(NodeId(running.node.clone())),
                part: Some("99999".into()),
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "missing pane reported", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub()
                .read(cx)
                .status
                .as_ref()
                .is_some_and(|status| status.failure && status.text.contains("no pane 99999"))
        })
    });
    assert_eq!(
        page.read_with(cx, |p, _| p.settings.workspace.clone()),
        before
    );
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn work_item_in_terminal_opens_beside_and_focus_is_published(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-work-link") else {
        return;
    };
    let (shell, cx, settings) = open_with_alpha(&running, "work-link", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    let toy = shell.read_with(cx, |s, _| Host::new(s.hub()).for_page("toy", &[]));
    let sessions = cx.update(|_, cx| toy.model::<SessionList>(cx)).unwrap();
    until(cx, "alpha focused in the shared model", |cx| {
        sessions.read_with(cx, |list, _| {
            list.focused
                .as_ref()
                .is_some_and(|f| f.node == running.node && f.user.is_empty() && f.name == "alpha")
        })
    });
    tmux(
        &running,
        &[
            "send-keys",
            "-t",
            "alpha",
            "printf '\\033[2J\\033[Hparvi-pnu.7.3\\n'",
            "Enter",
        ],
    );
    until(cx, "work id printed", |cx| {
        page.read_with(cx, |p, _| {
            p.window_text(0)
                .is_some_and(|t| t.lines().any(|l| l.trim() == "parvi-pnu.7.3"))
        })
    });
    let pos = page.read_with(cx, |p, _| {
        let text = p.window_text(0).unwrap();
        let row = text
            .lines()
            .position(|line| line.trim() == "parvi-pnu.7.3")
            .unwrap();
        let geo = p.geo.borrow()[0];
        let origin = geo.origin.unwrap();
        let cell = geo.cell.unwrap();
        gpui::point(
            origin.x + cell.width * 3. + px(2.),
            origin.y + cell.height * row as f32 + px(2.),
        )
    });
    ITEM_ASK.with(|slot| *slot.borrow_mut() = None);
    cx.simulate_click(pos, Modifiers::none());
    until(cx, "work item asked", |_| {
        ITEM_ASK.with(|slot| slot.borrow().is_some())
    });
    let ask = ITEM_ASK.with(|slot| slot.borrow().clone()).unwrap();
    assert_eq!(
        ask,
        Ask::Reveal {
            what: "item".into(),
            id: "parvi-pnu.7.3".into(),
            node: Some(NodeId(running.node.clone())),
            part: None,
            beside: true
        }
    );
    until(cx, "item beside terminals", |cx| {
        cx.debug_bounds("terminals-beside").is_some()
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.shown_page()),
        Some("terminals")
    );
    let beta = Key {
        node: running.node.clone(),
        user: String::new(),
        name: "beta".into(),
    };
    cx.update(|window, cx| {
        page.update(cx, |p, cx| {
            p.settings.workspace.set_windows(0, 2);
            p.attach(beta, 1, window, cx);
        })
    });
    until(cx, "beta focused in shared model", |cx| {
        sessions.read_with(cx, |list, _| {
            list.focused
                .as_ref()
                .is_some_and(|f| f.node == running.node && f.user.is_empty() && f.name == "beta")
        })
    });
    until(cx, "alpha tile painted", |cx| {
        page.read_with(cx, |p, _| p.geo.borrow()[0].body.is_some())
    });
    let alpha = page.read_with(cx, |p, _| p.geo.borrow()[0].body.unwrap().center());
    cx.simulate_click(alpha, Modifiers::none());
    until(cx, "click focuses alpha in shared model", |cx| {
        sessions.read_with(cx, |list, _| {
            list.focused
                .as_ref()
                .is_some_and(|f| f.node == running.node && f.user.is_empty() && f.name == "alpha")
        })
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// Keep the first launch's shell from becoming ready until after a later
/// unknown-node ask has reported its failure. The node has already listed
/// the new session, but has not answered the launch yet.
#[gpui::test]
fn late_launch_success_does_not_replace_a_newer_failure(cx: &mut TestAppContext) {
    let Some(running) = start_with_harness("terminals-launch-status-order", true) else {
        return;
    };
    let (shell, cx, settings) = open_with_alpha(&running, "launch-status-order", cx);
    let gate_shell = running.fixture.join("gate-shell");
    let tmux_program = std::env::var_os("PARVI_TMUX").unwrap_or_else(|| "tmux".into());
    parvi_testkit::tmux::write_script(
        &gate_shell,
        format!(
            "#!/bin/sh\n'{}' -S '{}' wait-for launch-status-release\nexec /bin/bash --noprofile --norc -i\n",
            std::path::PathBuf::from(tmux_program).display(),
            running.socket.display()
        ),
    );
    tmux(
        &running,
        &[
            "set-option",
            "-g",
            "default-shell",
            gate_shell.to_str().unwrap(),
        ],
    );
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    cx.update(|_, cx| {
        page.update(cx, |p, cx| {
            p.settings.last_harness = Some("fake".into());
            cx.notify();
        })
    });
    let toy = shell.read_with(cx, |s, _| Host::new(s.hub()).for_page("toy", &[]));
    let folder = settings.parent().unwrap().to_string_lossy().to_string();
    cx.update(|_, cx| {
        toy.ask(
            Ask::Launch {
                node: NodeId(running.node.clone()),
                folder,
                name: "alpha".into(),
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "first launch listed but unanswered", |cx| {
        page.read_with(cx, |p, _| {
            !p.launches.is_empty()
                && p.fleet
                    .all
                    .iter()
                    .any(|s| s.node == running.node && s.name == "alpha-2")
        })
    });
    cx.update(|_, cx| {
        toy.ask(
            Ask::Launch {
                node: NodeId("nowhere".into()),
                folder: "/".into(),
                name: "lost".into(),
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "newer failure reported", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).status.as_ref().is_some_and(|status| {
                status.failure
                    && status.text == "Could not start lost: its node is not in the fleet"
            })
        })
    });
    tmux(&running, &["wait-for", "-S", "launch-status-release"]);
    until(cx, "first launch answered", |cx| {
        page.read_with(cx, |p, _| p.launches.is_empty())
    });
    let status = shell.read_with(cx, |s, cx| s.hub().read(cx).status.clone());
    assert_eq!(
        status.map(|s| (s.text, s.failure)),
        Some((
            "Could not start lost: its node is not in the fleet".into(),
            true
        ))
    );
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}
