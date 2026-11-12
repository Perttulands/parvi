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
const AREAS_WITH_FILES: &[Listing] = &[parvi_terminals::LISTING, parvi_files::LISTING];

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
    const ANSWERS: &'static [Answers] = &[Answers::Reveal("item"), Answers::OpenPath];

    fn answer(&mut self, ask: Ask, from: &'static str, _: &mut Window, cx: &mut Context<Self>) {
        if ask.beside() {
            self.host
                .show_beside(from, cx.new(|cx| Note(cx.focus_handle())).into(), cx);
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
    start_config(label, harness, restart, false, false)
}

fn start_config(
    label: &str,
    harness: bool,
    restart: bool,
    files: bool,
    bash: bool,
) -> Option<Running> {
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
            if bash {
                for session in ["alpha", "beta"] {
                    tmux.run(&["respawn-pane", "-k", "-t", session, "env PS1='$ ' /bin/bash --noprofile --norc -i"]);
                }
            }
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
            let areas = if files { AREAS_WITH_FILES } else { AREAS };
            let config = if files { format!("{config}\n[files]\nroots = []\n") } else { config };
            let node = Node::start(areas, &config).await.expect("a node");
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
    // session of the window that last had focus, named in the drawer.
    until(cx, "the Send drawer", |cx| {
        cx.debug_bounds("send-go").is_some()
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.shown_page()),
        Some("terminals")
    );
    let page = PAGE.with(|p| p.borrow().clone()).expect("the page");
    let aimed = page.read_with(cx, |p, _| p.send_target.clone());
    assert_eq!(aimed.map(|k| k.name), Some("alpha".to_string()));
    assert!(
        cx.debug_bounds("send-aimed-alpha").is_some(),
        "named at the top"
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

struct Note(gpui::FocusHandle);

impl Render for Note {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().track_focus(&self.0).child("beside")
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
    let note = cx.update(|_, cx| cx.new(|cx| Note(cx.focus_handle())));
    cx.update(|_, cx| toy.show_beside("terminals", note.clone().into(), cx));
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("terminals-beside").is_some(),
        "drawn beside"
    );
    cx.update(|window, cx| window.focus(&note.read(cx).0.clone(), cx));
    cx.update(|_, cx| toy.close_beside("terminals", cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("terminals-beside").is_none(), "closed");
    // The view lives on, holding the focus; the terminal takes it back.
    let page = PAGE.with(|p| p.borrow().clone()).expect("the page");
    assert!(cx.update(|window, cx| page.read(cx).term_focus.is_focused(window)));
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
    // All four panes must receive their screens without clicking a tile.
    until(cx, "all four newly launched screens", |cx| {
        page.read_with(cx, |p, _| {
            (0..4).all(|i| p.window_text(i).is_some_and(|text| !text.trim().is_empty()))
        })
    });
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
    let submitted = running.fixture.join("send-was-submitted");
    let reference = format!("touch {}", submitted.display());
    cx.update(|_, cx| {
        host.ask(
            Ask::Send {
                reference: reference.clone(),
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
    until(cx, "new agent paste completed", |cx| {
        cx.debug_bounds("send-body").is_none()
    });
    until(cx, "reference pasted into the new agent", |_cx| {
        tmux(&running, &["capture-pane", "-p", "-t", &new_name]).contains(&reference)
    });
    assert!(
        !submitted.exists(),
        "new agent must not submit the pasted command"
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
    scroll_to_launch(cx, "launcher-scroll-0");
    until(cx, "visible launch button", |cx| {
        cx.debug_bounds("launch-0")
            .zip(cx.debug_bounds("launcher-scroll-0"))
            .is_some_and(|(button, viewport)| viewport.contains(&button.center()))
    });
    click(cx, "launch-0".into());
    until(cx, "selected flag reaches the harness", |_| {
        std::fs::read_to_string(running.fixture.join("runs"))
            .is_ok_and(|runs| runs.contains("--verbose"))
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

/// The shell routes the leader and Escape while a real terminal is attached.
#[gpui::test]
fn leader_and_escape_route_around_an_attached_terminal(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-keys-journey") else {
        return;
    };
    let (shell, cx, settings) = open_with_alpha(&running, "keys-journey", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    cx.simulate_keystrokes("ctrl-shift-space s");
    until(cx, "leader opens Send", |cx| {
        cx.debug_bounds("send-body").is_some()
    });
    cx.simulate_keystrokes("escape");
    until(cx, "Escape closes Send", |cx| {
        cx.debug_bounds("send-body").is_none()
    });
    assert!(!page.read_with(cx, |p, _| p.is_open("send")));
    let escape_file = running.fixture.join("escape-byte");
    tmux(
        &running,
        &[
            "send-keys",
            "-t",
            "alpha",
            &format!(
                "stty raw -echo; dd bs=1 count=1 of='{}' 2>/dev/null; stty sane",
                escape_file.display()
            ),
            "Enter",
        ],
    );
    until(cx, "terminal waiting for a byte", |_| {
        tmux(
            &running,
            &[
                "display-message",
                "-p",
                "-t",
                "alpha",
                "#{pane_current_command}",
            ],
        )
        .trim()
            == "dd"
    });
    cx.simulate_keystrokes("escape");
    until(cx, "Escape reaches the terminal", |_| {
        std::fs::read(&escape_file).is_ok_and(|b| b == [27])
    });
    // A Finnish keyboard produces plus without Shift.
    cx.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke {
            key: "+".into(),
            key_char: Some("+".into()),
            modifiers: Modifiers {
                alt: true,
                ..Modifiers::none()
            },
        },
        is_held: false,
        prefer_character_input: false,
    });
    until(cx, "Finnish Alt-plus adds a window", |cx| {
        page.read_with(cx, |p, _| p.settings.workspace.windows().len() == 2)
    });
    cx.simulate_keystrokes("ctrl-shift-space k");
    until(cx, "keys off", |cx| {
        shell.read_with(cx, |s, cx| !s.hub().read(cx).settings.keys)
    });
    cx.simulate_keystrokes("alt-s");
    cx.run_until_parked();
    assert!(cx.debug_bounds("send-body").is_none());
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn peek_keyboard_dismisses_and_restores_focus_after_a_leader_action(cx: &mut TestAppContext) {
    let Some(running) = start("peek-keyboard-focus") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "peek-keyboard-focus", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    until(cx, "beta listed", |cx| {
        page.read_with(cx, |p, _| p.fleet.all.iter().any(|s| s.name == "beta"))
    });
    cx.simulate_keystrokes("ctrl-shift-space /");
    cx.simulate_input("beta");
    cx.simulate_keystrokes("enter");
    until(cx, "keyboard opens Peek", |cx| {
        cx.debug_bounds("peek-attach").is_some()
    });
    cx.simulate_keystrokes("escape");
    until(cx, "Escape closes Peek", |cx| {
        cx.debug_bounds("peek-attach").is_none()
    });
    cx.simulate_input("echo peek-escape-restored");
    cx.simulate_keystrokes("enter");
    until(cx, "typing returns to the terminal", |cx| {
        shown(cx).contains("peek-escape-restored")
    });

    cx.simulate_keystrokes("ctrl-shift-space /");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("beta");
    cx.simulate_keystrokes("enter");
    until(cx, "keyboard reopens Peek", |cx| {
        cx.debug_bounds("peek-attach").is_some()
    });
    cx.simulate_keystrokes("ctrl-shift-space s");
    until(cx, "leader opens Send over Peek", |cx| {
        cx.debug_bounds("send-drawer").is_some()
    });
    cx.simulate_keystrokes("escape");
    until(cx, "Escape returns to Peek", |cx| {
        cx.debug_bounds("send-drawer").is_none() && cx.debug_bounds("peek-attach").is_some()
    });
    cx.simulate_keystrokes("ctrl-shift-space v");
    until(cx, "leader opens Overview over Peek", |cx| {
        cx.debug_bounds("overview-grid").is_some()
    });
    cx.simulate_keystrokes("escape");
    until(cx, "Overview returns focus to Peek", |cx| {
        cx.debug_bounds("overview-grid").is_none() && cx.debug_bounds("peek-attach").is_some()
    });
    cx.simulate_keystrokes("escape");
    until(cx, "second Escape closes Peek", |cx| {
        cx.debug_bounds("peek-attach").is_none()
    });
    cx.simulate_input("echo peek-leader-restored");
    cx.simulate_keystrokes("enter");
    until(cx, "typing returns after nested layer", |cx| {
        shown(cx).contains("peek-leader-restored")
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn overview_keyboard_dismisses_selects_and_routes_leader(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-overview-keys") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "overview-keys", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    until(cx, "both sessions in the overview", |cx| {
        page.read_with(cx, |p, _| p.fleet.all.len() == 2)
    });

    cx.simulate_keystrokes("alt-v");
    until(cx, "overview open", |cx| {
        cx.debug_bounds("overview-grid").is_some()
    });
    cx.simulate_keystrokes("escape");
    until(cx, "Escape closes overview", |cx| {
        cx.debug_bounds("overview-grid").is_none()
    });

    cx.simulate_keystrokes("alt-v");
    until(cx, "overview reopened", |cx| {
        cx.debug_bounds("overview-grid").is_some()
    });
    cx.simulate_keystrokes("ctrl-shift-space v");
    until(cx, "leader closes overview", |cx| {
        cx.debug_bounds("overview-grid").is_none()
    });

    cx.simulate_keystrokes("alt-v");
    until(cx, "overview reopened for selection", |cx| {
        cx.debug_bounds("overview-grid").is_some()
    });
    let (index, wanted) = page.read_with(cx, |p, _| {
        let index = p.fleet.all.iter().position(|s| s.name == "beta").unwrap();
        (index, Key::of(&p.fleet.all[index]))
    });
    cx.simulate_keystrokes("right");
    until(cx, "arrow selects second session", |cx| {
        page.read_with(cx, |p, _| p.overview_cursor == 1)
    });
    cx.simulate_keystrokes("left");
    until(cx, "left selects the first session", |cx| {
        page.read_with(cx, |p, _| p.overview_cursor == 0)
    });
    cx.simulate_keystrokes("down");
    until(cx, "down selects the next row", |cx| {
        page.read_with(cx, |p, _| p.overview_cursor == 1)
    });
    cx.simulate_keystrokes("up");
    until(cx, "up selects the previous row", |cx| {
        page.read_with(cx, |p, _| p.overview_cursor == 0)
    });
    if index == 1 {
        cx.simulate_keystrokes("right");
    }
    cx.simulate_keystrokes("enter");
    until(cx, "Enter opens selected session", |cx| {
        cx.debug_bounds("overview-grid").is_none()
            && page.read_with(cx, |p, _| p.settings.workspace.bound(&wanted).is_some())
    });
    until(cx, "selected session is ready", |cx| {
        page.read_with(cx, |p, _| {
            p.window_text(p.settings.workspace.focus)
                .is_some_and(|text| text.contains('$'))
        })
    });
    cx.simulate_input("echo overview-keyboard-restored");
    cx.simulate_keystrokes("enter");
    until(cx, "typing reaches selected session", |_| {
        tmux(&running, &["capture-pane", "-p", "-t", "beta"]).contains("overview-keyboard-restored")
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// A clipboard image travels through Send to the target node before its path
/// and the originating reference are delivered to the session.
#[gpui::test]
fn pasted_attachment_reaches_the_node_and_session(cx: &mut TestAppContext) {
    let Some(running) = start_config("terminals-pasted-attachment", false, false, true, false)
    else {
        return;
    };
    let received = running.fixture.join("received");
    tmux(
        &running,
        &[
            "send-keys",
            "-t",
            "alpha",
            &format!("cat > '{}'", received.display()),
            "Enter",
        ],
    );
    let (shell, cx, settings) = open_with_alpha(&running, "pasted-attachment", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    let host = shell.read_with(cx, |s, _| Host::new(s.hub()).for_page("toy", &[]));
    cx.update(|_, cx| {
        host.ask(
            Ask::Send {
                reference: "file /example.txt".into(),
                target: None,
            },
            cx,
        )
    })
    .unwrap();
    until(cx, "Send drawer", |cx| cx.debug_bounds("send-go").is_some());
    let bytes = vec![1, 2, 3, 4, 5];
    let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes.clone());
    cx.write_to_clipboard(gpui::ClipboardItem::new_image(&image));
    cx.update(|window, cx| {
        page.update(cx, |p, cx| {
            parvi_app_kit::widgets::field::focus(&p.send_note, window, cx);
        })
    });
    cx.simulate_keystrokes("ctrl-v");
    until(cx, "clipboard attachment", |cx| {
        page.read_with(cx, |p, _| !p.send.files.is_empty())
    });
    click(cx, "send-go".into());
    let mut path = std::path::PathBuf::new();
    until(cx, "reference and drop path delivered", |_| {
        let Ok(text) = std::fs::read_to_string(&received) else {
            return false;
        };
        if !text.contains("file /example.txt") {
            return false;
        }
        let Some(drop) = text.lines().find_map(|line| line.strip_prefix("- ")) else {
            return false;
        };
        path = drop.into();
        path.exists()
    });
    assert!(path.to_string_lossy().contains("drops"));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// Hooks mark both the session and its tab until a CLI acknowledgement.
#[gpui::test]
fn hook_reports_mark_session_and_tab_until_cli_seen(cx: &mut TestAppContext) {
    struct ToneLog;
    static TONES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    static LOGGER: ToneLog = ToneLog;
    impl log::Log for ToneLog {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            if record.target() == "parvi_terminals_app::sound" {
                TONES.lock().unwrap().push(record.args().to_string());
            }
        }
        fn flush(&self) {}
    }
    log::set_logger(&LOGGER).expect("install the tone observer");
    log::set_max_level(log::LevelFilter::Warn);
    let Some(running) = start("terminals-hook-journey") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "hook-journey", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    cx.update(|_, cx| page.update(cx, |p, _| p.settings.event_tone = true));
    let key = Key {
        node: running.node.clone(),
        user: String::new(),
        name: "alpha".into(),
    };
    let id = page.read_with(cx, |p, _| p.fleet.session(&key).unwrap().id.clone());
    // The terminal is not being looked at, so a report remains news.
    click(cx, "tab-toy".into());
    for (event, kind) in [
        ("finished", parvi_terminals_contract::EventKind::Finished),
        (
            "needs-input",
            parvi_terminals_contract::EventKind::NeedsInput,
        ),
    ] {
        let url = format!(
            "{}/api/agent/event?session=%24{}&event={event}",
            running.url,
            &id[1..]
        );
        parvi_node_kit::hook::report(&[url, "{\"message\":\"ready\"}".into()]);
        until(cx, "hook marks the row and tab", |cx| {
            page.read_with(cx, |p, _| {
                p.fleet
                    .session(&key)
                    .and_then(|s| p.marked(s))
                    .is_some_and(|e| e.kind == kind)
                    && p.tab_marked(0)
            })
        });
        until(cx, "tone requested on the test platform", |_| {
            TONES
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains(&format!("tone {event} not played")))
        });
        let args = ["seen", "alpha", "--node", &running.url].map(str::to_owned);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(
            runtime
                .block_on(parvi_node_kit::cli::run("session", &args, &mut Vec::new()))
                .unwrap(),
            0
        );
        until(cx, "seen clears the row and tab", |cx| {
            page.read_with(cx, |p, _| {
                p.fleet.session(&key).is_some_and(|s| p.marked(s).is_none()) && !p.tab_marked(0)
            })
        });
    }
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// Showing a session is looking at it, whether or not a window manager says
/// the app's window is active: the test window never is, like a bare display.
#[gpui::test]
fn clicking_a_marked_session_clears_its_mark(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-mark-click") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "mark-click", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    let key = Key {
        node: running.node.clone(),
        user: String::new(),
        name: "alpha".into(),
    };
    let id = page.read_with(cx, |p, _| p.fleet.session(&key).unwrap().id.clone());
    let url = format!(
        "{}/api/agent/event?session=%24{}&event=needs-input",
        running.url,
        &id[1..]
    );
    parvi_node_kit::hook::report(&[url, "{\"message\":\"ready\"}".into()]);
    until(cx, "the report marks the row", |cx| {
        page.read_with(cx, |p, _| {
            p.fleet.session(&key).is_some_and(|s| p.marked(s).is_some())
        })
    });
    click(cx, format!("row-{}::alpha", running.node));
    until(cx, "showing the session clears its mark", |cx| {
        page.read_with(cx, |p, _| {
            p.fleet.session(&key).is_some_and(|s| p.marked(s).is_none())
        })
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// Alt+N with every window of the tab holding a session opens the launcher
/// in a new tab and leaves those windows as they were.
#[gpui::test]
fn the_launcher_opens_a_new_tab_when_the_windows_are_full(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-launch-full") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "launch-full", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    let full: Vec<Key> = (0..4)
        .map(|i| Key {
            node: running.node.clone(),
            user: String::new(),
            name: format!("held-{i}"),
        })
        .collect();
    cx.update(|_, cx| {
        page.update(cx, |p, _| {
            let ws = &mut p.settings.workspace;
            ws.set_windows(0, 4);
            for (w, key) in full.iter().enumerate() {
                ws.bind_in(0, w, key.clone(), None);
            }
        })
    });
    cx.simulate_keystrokes("alt-n");
    page.read_with(cx, |p, _| {
        let ws = &p.settings.workspace;
        assert_eq!((ws.count, ws.active), (2, 1), "a new tab is shown");
        assert!(ws.windows()[ws.focus].is_empty(), "its window is free");
        assert!(
            ws.tabs[0].windows.iter().take(4).all(|w| !w.is_empty()),
            "the full tab is kept"
        );
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// Links emitted by a scratch tmux session reach the platform and the page
/// that answers paths, including a path wrapped by the terminal grid.
#[gpui::test]
fn terminal_links_open_url_and_wrapped_quoted_path(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-link-journey") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "link-journey", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    let path = format!("/tmp/{}/leaf.txt", "long".repeat(55));
    let command =
        format!("printf '\\033[2J\\033[H\"{path}\"\\nhttps://example.test/safe\\n🙂X\\n'");
    tmux(&running, &["send-keys", "-t", "alpha", &command, "Enter"]);
    until(cx, "terminal links and emoji printed", |cx| {
        let text = shown(cx);
        text.lines()
            .any(|line| line.trim() == "https://example.test/safe")
            && text.lines().any(|line| line.trim() == "🙂 X")
    });
    let position = |cx: &mut VisualTestContext, needle: &str| {
        page.read_with(cx, |p, _| {
            let text = p.window_text(0).unwrap();
            let (row, line) = text
                .lines()
                .enumerate()
                .find(|(_, line)| line.contains(needle))
                .unwrap();
            let col = line.find(needle).unwrap();
            let geo = p.geo.borrow()[0];
            let origin = geo.origin.unwrap();
            let cell = geo.cell.unwrap();
            gpui::point(
                origin.x + cell.width * col as f32 + px(2.),
                origin.y + cell.height * row as f32 + px(2.),
            )
        })
    };
    let url = position(cx, "https://example.test/safe");
    cx.simulate_click(url, Modifiers::none());
    until(cx, "URL opened", |cx| {
        cx.opened_url().as_deref() == Some("https://example.test/safe")
    });
    ITEM_ASK.with(|slot| *slot.borrow_mut() = None);
    let path_pos = position(cx, "/tmp/");
    cx.simulate_click(path_pos, Modifiers::none());
    until(cx, "wrapped path opened", |_| {
        ITEM_ASK.with(|slot| matches!(&*slot.borrow(), Some(Ask::OpenPath { path: got, beside: true, .. }) if got == &path))
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn selection_copies_when_mouse_releases_outside_the_tile(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-selection-journey") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "selection-journey", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    tmux(
        &running,
        &[
            "send-keys",
            "-t",
            "alpha",
            "printf '\\033[2J\\033[Hcopy-marker\\n'",
            "Enter",
        ],
    );
    until(cx, "selection text", |cx| {
        shown(cx).lines().any(|line| line.trim() == "copy-marker")
    });
    let (start, end) = page.read_with(cx, |p, _| {
        let text = p.window_text(0).unwrap();
        let (row, line) = text
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains("copy-marker"))
            .unwrap();
        let col = line.find("copy-marker").unwrap();
        let geo = p.geo.borrow()[0];
        let origin = geo.origin.unwrap();
        let cell = geo.cell.unwrap();
        (
            gpui::point(
                origin.x + cell.width * col as f32 + px(2.),
                origin.y + cell.height * row as f32 + px(2.),
            ),
            gpui::point(
                origin.x + cell.width * (col + 11) as f32 + px(2.),
                origin.y + cell.height * row as f32 + px(2.),
            ),
        )
    });
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(end, gpui::MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(
        gpui::point(px(0.), px(0.)),
        gpui::MouseButton::Left,
        Modifiers::none(),
    );
    until(cx, "selection copied", |cx| {
        cx.read_from_clipboard()
            .and_then(|c| c.text())
            .is_some_and(|s| s.contains("copy-marker"))
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// Arrange a real session using the same page actions as the tab menus and
/// drag another session from the rail into an empty window.
#[gpui::test]
fn arrange_tabs_preset_drag_and_claim_on_a_live_node(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-arrange-journey") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "arrange-journey", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    let run_item = |cx: &mut VisualTestContext, tab: usize, label: &str| {
        let action = page.read_with(cx, |p, _| {
            p.tab_menu(tab)
                .into_iter()
                .find(|r| r.label == label)
                .unwrap()
                .act
                .unwrap()
        });
        cx.update(|window, cx| page.update(cx, |p, cx| p.act(action, window, cx)));
    };
    let two = page.read_with(cx, |p, _| {
        p.tab_menu(0)
            .into_iter()
            .find(|r| r.label == "Windows")
            .unwrap()
            .sub
            .into_iter()
            .find(|r| r.label == "2")
            .unwrap()
            .act
            .unwrap()
    });
    cx.update(|window, cx| page.update(cx, |p, cx| p.act(two, window, cx)));
    until(cx, "second window", |cx| {
        cx.debug_bounds("launcher-scroll-1").is_some()
    });
    let beta: &'static str = Box::leak(format!("row-{}::beta", running.node).into_boxed_str());
    until(cx, "beta row", |cx| cx.debug_bounds(beta).is_some());
    let from = cx.debug_bounds(beta).unwrap().center();
    let to = cx.debug_bounds("launcher-scroll-1").unwrap().center();
    cx.simulate_mouse_down(from, gpui::MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        gpui::point(from.x + px(10.), from.y),
        gpui::MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(to, gpui::MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(to, gpui::MouseButton::Left, Modifiers::none());
    until(cx, "beta bound by dragging", |cx| {
        page.read_with(cx, |p, _| {
            p.settings
                .workspace
                .shown(1)
                .is_some_and(|b| b.key.name == "beta")
        })
    });
    run_item(cx, 0, "Save layout as preset");
    cx.simulate_input("Morning");
    cx.simulate_keystrokes("enter");
    until(cx, "preset saved", |cx| {
        page.read_with(cx, |p, _| {
            p.settings
                .workspace
                .presets
                .iter()
                .any(|p| p.name == "Morning")
        })
    });
    until(cx, "tab add control", |cx| {
        cx.debug_bounds("tab-add").is_some()
    });
    click(cx, "tab-add".into());
    until(cx, "second tab", |cx| {
        cx.debug_bounds("tab-terminals-1").is_some()
    });
    run_item(cx, 1, "Rename tab");
    cx.simulate_input("Ops");
    cx.simulate_keystrokes("enter");
    until(cx, "tab renamed", |cx| {
        page.read_with(cx, |p, _| {
            p.settings.workspace.tabs[1].name.as_deref() == Some("Ops")
        })
    });
    run_item(cx, 1, "Close tab");
    until(cx, "tab removed", |cx| {
        page.read_with(cx, |p, _| p.settings.workspace.shown_tabs().len() == 1)
    });
    let restore = page.read_with(cx, |p, _| {
        p.tab_menu(0)
            .into_iter()
            .find(|r| r.label == "Restore preset")
            .unwrap()
            .sub
            .into_iter()
            .find(|r| r.label == "Morning")
            .unwrap()
            .act
            .unwrap()
    });
    cx.update(|window, cx| page.update(cx, |p, cx| p.act(restore, window, cx)));
    until(cx, "preset restores beta", |cx| {
        page.read_with(cx, |p, _| {
            p.settings
                .workspace
                .shown(1)
                .is_some_and(|b| b.key.name == "beta")
        })
    });
    cx.simulate_keystrokes("alt-c");
    until(cx, "Fit every window reaches tmux", |cx| {
        page.read_with(cx, |p, _| {
            p.fleet
                .all
                .iter()
                .filter(|s| s.name == "alpha" || s.name == "beta")
                .all(|s| s.seat == Some(parvi_terminals_contract::Seat::Mine))
        })
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// The offered recent folder is completed by Tab and used by the fake harness.
#[gpui::test]
fn launcher_completes_recent_folder_before_starting_agent(cx: &mut TestAppContext) {
    let Some(running) = start_with_restart_harness("terminals-folder-journey", true, true) else {
        return;
    };
    let settings = parvi_testkit::fresh_dir("folder-journey-settings").join("app.json");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (url, path) = (running.url.clone(), settings.clone());
    let (_shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[terminals], Some(url), Some(path), window, cx)
    });
    until(cx, "fake harness offered", |cx| {
        cx.debug_bounds("harness-0-fake").is_some()
    });
    click(cx, "harness-0-fake".into());
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    cx.update(|window, cx| {
        page.update(cx, |p, cx| {
            p.launchers[0].update(cx, |l, cx| l.focus_first(window, cx))
        })
    });
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("flags-ui-");
    cx.simulate_keystrokes("tab");
    scroll_to_launch(cx, "launcher-scroll-0");
    until(cx, "launch available", |cx| {
        cx.debug_bounds("launch-0")
            .zip(cx.debug_bounds("launcher-scroll-0"))
            .is_some_and(|(button, viewport)| viewport.contains(&button.center()))
    });
    click(cx, "launch-0".into());
    let expected = format!("{}|--marker", running.fixture.display());
    until(cx, "agent started in the completed folder", |_| {
        std::fs::read_to_string(running.fixture.join("runs")).is_ok_and(|s| s.contains(&expected))
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// Send opened from alpha carries its identity when redirected to beta.
#[gpui::test]
fn terminal_send_carries_source_reference_to_another_session(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-send-source") else {
        return;
    };
    let received = running.fixture.join("source-reference");
    tmux(
        &running,
        &[
            "send-keys",
            "-t",
            "beta",
            &format!("cat > '{}'", received.display()),
            "Enter",
        ],
    );
    let (_shell, cx, settings) = open_with_alpha(&running, "send-source", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    until(cx, "beta listed", |cx| {
        page.read_with(cx, |p, _| p.fleet.all.iter().any(|s| s.name == "beta"))
    });
    cx.simulate_keystrokes("alt-s");
    until(cx, "Send opened from terminal", |cx| {
        cx.debug_bounds("send-go").is_some()
    });
    let beta = format!("send-target-{}::beta", running.node);
    let selector: &'static str = Box::leak(beta.clone().into_boxed_str());
    until(cx, "beta target", |cx| cx.debug_bounds(selector).is_some());
    click(cx, beta);
    click(cx, "send-go".into());
    let expected = format!("session {}::alpha", running.node);
    until(cx, "source reference delivered to beta", |_| {
        std::fs::read_to_string(&received).is_ok_and(|s| s.contains(&expected))
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn window_send_uses_the_clicked_window_as_source(cx: &mut TestAppContext) {
    let Some(running) = start("window-send-source") else {
        return;
    };
    let received = running.fixture.join("window-source");
    tmux(
        &running,
        &[
            "send-keys",
            "-t",
            "beta",
            &format!("cat > '{}'", received.display()),
            "Enter",
        ],
    );
    let (_shell, cx, settings) = open_with_alpha(&running, "window-send-source", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    let beta = Key {
        node: running.node.clone(),
        user: String::new(),
        name: "beta".into(),
    };
    until(cx, "beta is listed", |cx| {
        page.read_with(cx, |p, _| p.fleet.all.iter().any(|s| s.name == "beta"))
    });
    cx.update(|window, cx| page.update(cx, |p, cx| p.place(beta.clone(), window, cx)));
    until(cx, "beta focused beside alpha", |cx| {
        page.read_with(cx, |p, _| p.settings.workspace.focus == 1)
    });
    click(cx, "window-send-0".into());
    until(cx, "source picker", |cx| {
        cx.debug_bounds("send-go").is_some()
    });
    page.read_with(cx, |p, _| {
        assert_eq!(
            p.send.reference.as_deref(),
            Some(format!("session {}::alpha", running.node).as_str())
        );
        assert!(
            p.send_target.is_none(),
            "a window must not target itself by default"
        );
    });
    let alpha = format!("send-target-{}::alpha", running.node);
    let selector: &'static str = Box::leak(alpha.into_boxed_str());
    assert!(
        cx.debug_bounds(selector).is_none(),
        "the source is not offered as a target"
    );
    let target = format!("send-target-{}::beta", running.node);
    click(cx, target);
    click(cx, "send-go".into());
    let expected = format!("session {}::alpha", running.node);
    until(cx, "source reference reaches beta", |_| {
        std::fs::read_to_string(&received).is_ok_and(|s| s.contains(&expected))
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn absent_tab_chord_does_not_reach_the_terminal(cx: &mut TestAppContext) {
    let Some(running) = start("absent-tab-key") else {
        return;
    };
    let (shell, cx, settings) = open_with_alpha(&running, "absent-tab-key", cx);
    let received = running.fixture.join("key-byte");
    tmux(
        &running,
        &[
            "send-keys",
            "-t",
            "alpha",
            &format!(
                "stty raw -echo; dd bs=1 count=1 of='{}' 2>/dev/null; stty sane",
                received.display()
            ),
            "Enter",
        ],
    );
    until(cx, "terminal waiting for one byte", |_| {
        tmux(
            &running,
            &[
                "display-message",
                "-p",
                "-t",
                "alpha",
                "#{pane_current_command}",
            ],
        )
        .trim()
            == "dd"
    });
    cx.simulate_keystrokes("alt-2");
    cx.simulate_input("x");
    until(cx, "ordinary input reaches terminal", |_| {
        std::fs::read(&received).is_ok_and(|b| b == b"x")
    });
    until(cx, "shell restored", |_| {
        tmux(
            &running,
            &[
                "display-message",
                "-p",
                "-t",
                "alpha",
                "#{pane_current_command}",
            ],
        )
        .trim()
            == "sh"
    });
    let raw = running.fixture.join("keys-off-byte");
    tmux(
        &running,
        &[
            "send-keys",
            "-t",
            "alpha",
            &format!(
                "stty raw -echo; dd bs=1 count=1 of='{}' 2>/dev/null; stty sane",
                raw.display()
            ),
            "Enter",
        ],
    );
    until(cx, "terminal waiting again", |_| {
        tmux(
            &running,
            &[
                "display-message",
                "-p",
                "-t",
                "alpha",
                "#{pane_current_command}",
            ],
        )
        .trim()
            == "dd"
    });
    cx.simulate_keystrokes("ctrl-shift-space k");
    until(cx, "keys off", |cx| {
        shell.read_with(cx, |s, cx| !s.hub().read(cx).settings.keys)
    });
    cx.simulate_keystrokes("alt-2");
    until(cx, "keys off passes Alt to terminal", |_| {
        std::fs::read(&raw).is_ok_and(|b| b == [27])
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn search_shows_and_marks_a_wrapped_history_match(cx: &mut TestAppContext) {
    let Some(running) = start("search-marked-history") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "search-marked-history", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    tmux(
        &running,
        &[
            "send-keys",
            "-t",
            "alpha",
            "printf '%0300d\\n' 0; printf '%0300dmatch%s\\n' 0 'needle'",
            "Enter",
        ],
    );
    until(cx, "output reached the terminal", |cx| {
        shown(cx).contains("matchneedle")
    });
    cx.simulate_resize(gpui::size(px(1440.), px(900.)));
    cx.simulate_keystrokes("alt-h");
    until(cx, "search drawer opened beside the terminal", |cx| {
        let Some(drawer) = cx.debug_bounds("find-drawer") else {
            return false;
        };
        page.read_with(cx, |p, _| {
            p.geo.borrow()[0]
                .body
                .is_some_and(|body| body.origin.x + body.size.width <= drawer.origin.x)
        })
    });
    cx.simulate_input("matchneedle");
    until(cx, "search result", |cx| {
        cx.debug_bounds("find-match-0").is_some()
    });
    click(cx, "find-match-0".into());
    until(cx, "matched line selected in the terminal", |cx| {
        page.read_with(cx, |p, _| {
            p.histories
                .get(&p.settings.workspace.focus)
                .is_some_and(|h| {
                    let screen = h.screen.lock();
                    screen.text().contains("matchneedle")
                        && screen
                            .term
                            .selection_to_string()
                            .is_some_and(|text| text.contains("matchneedle"))
                })
        })
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

#[gpui::test]
fn send_drawer_keeps_the_terminal_visible_beside_it(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-send-layout") else {
        return;
    };
    let (_shell, cx, settings) = open_with_alpha(&running, "send-layout", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    cx.simulate_resize(gpui::size(px(1440.), px(900.)));
    until(cx, "terminal geometry", |cx| {
        page.read_with(cx, |p, _| p.geo.borrow()[0].body.is_some())
    });
    cx.simulate_keystrokes("alt-s");
    until(cx, "drawer beside terminal", |cx| {
        let Some(drawer) = cx.debug_bounds("send-drawer") else {
            return false;
        };
        page.read_with(cx, |p, _| {
            p.geo.borrow()[0]
                .body
                .is_some_and(|body| body.origin.x + body.size.width <= drawer.origin.x)
        })
    });
    assert!(!shown(cx).trim().is_empty());
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// A second node following the same scratch server is not a sizing viewer.
#[gpui::test]
fn opening_a_session_sizes_it_with_another_node_following(cx: &mut TestAppContext) {
    let Some(running) = start("terminals-second-node") else {
        return;
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let config = format!(
        "[terminals]\nsocket = {:?}\nlaunch = {:?}\n",
        running.socket,
        running.fixture.join("no-launch.conf")
    );
    let second = runtime.block_on(Node::start(AREAS, &config)).unwrap();
    runtime.block_on(parvi_testkit::tmux::eventually(
        "second node follows alpha",
        || {
            second
                .sessions()
                .list()
                .iter()
                .find(|s| s.name == "alpha" && !s.panes.is_empty())
                .map(|_| ())
        },
    ));
    let second_sessions = second.sessions();
    runtime.block_on(parvi_testkit::tmux::eventually(
        "second node attaches only to alpha",
        || {
            let sessions = second_sessions.list();
            let pane = sessions
                .iter()
                .find(|s| s.name == "alpha")?
                .panes
                .first()?
                .0;
            second_sessions.screen(pane).map(|_| ())
        },
    ));
    let (_shell, cx, settings) = open_with_alpha(&running, "second-node", cx);
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    until(cx, "session claims the tile at its grid", |cx| {
        page.read_with(cx, |p, _| {
            let Some(binding) = p.settings.workspace.shown(0) else {
                return false;
            };
            let Some(session) = p.fleet.session(&binding.key) else {
                return false;
            };
            let Some(grid) = p.launch_grids.borrow()[0] else {
                return false;
            };
            session.foreign == Some(0)
                && session.seat == Some(parvi_terminals_contract::Seat::Mine)
                && tmux(
                    &running,
                    &[
                        "display-message",
                        "-p",
                        "-t",
                        "alpha",
                        "#{window_width}x#{window_height}",
                    ],
                ) == format!("{}x{}", grid.0, grid.1)
        })
    });
    runtime.block_on(second.stop());
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// Completion reads the launch user's filesystem even outside Files roots.
#[gpui::test]
fn launcher_completes_a_folder_outside_files_roots(cx: &mut TestAppContext) {
    let Some(running) = start_with_restart_harness("terminals-folder-outside", true, true) else {
        return;
    };
    let chosen = running.fixture.join("subfolder");
    std::fs::create_dir(&chosen).unwrap();
    let settings = parvi_testkit::fresh_dir("folder-outside-settings").join("app.json");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (url, path) = (running.url.clone(), settings.clone());
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[terminals], Some(url), Some(path), window, cx)
    });
    until(cx, "fake harness offered", |cx| {
        cx.debug_bounds("harness-0-fake").is_some()
    });
    click(cx, "harness-0-fake".into());
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    cx.update(|window, cx| {
        page.update(cx, |p, cx| {
            p.launchers[0].update(cx, |l, cx| l.focus_first(window, cx))
        })
    });
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input(&format!("{}/sub", running.fixture.display()));
    let row: &'static str =
        Box::leak(format!("folder-row-0-{}", chosen.display()).into_boxed_str());
    until(cx, "folder completion", |cx| cx.debug_bounds(row).is_some());
    until(cx, "button names the typed folder", |cx| {
        cx.debug_bounds("launch-label-0-Launch fake in sub")
            .is_some()
    });
    cx.simulate_keystrokes("tab");
    until(cx, "button names the completed folder", |cx| {
        cx.debug_bounds("launch-label-0-Launch fake in subfolder")
            .is_some()
    });
    scroll_to_launch(cx, "launcher-scroll-0");
    until(cx, "launch available", |cx| {
        cx.debug_bounds("launch-0")
            .zip(cx.debug_bounds("launcher-scroll-0"))
            .is_some_and(|(button, viewport)| viewport.contains(&button.center()))
    });
    click(cx, "launch-0".into());
    let expected = format!("{}|--marker", chosen.display());
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        let runs = std::fs::read_to_string(running.fixture.join("runs")).unwrap_or_default();
        if runs.lines().any(|line| {
            line.split_once('|').is_some_and(|(folder, flags)| {
                std::path::Path::new(folder).canonicalize().ok() == chosen.canonicalize().ok()
                    && flags == "--marker"
            })
        }) {
            break;
        }
        let status = shell.read_with(cx, |s, cx| {
            s.hub().read(cx).status.as_ref().map(|s| s.text.clone())
        });
        assert!(
            Instant::now() < deadline,
            "expected {expected:?}, runs {runs:?}, status {status:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

/// Opening the app and following idle sessions must not make them look busy.
#[gpui::test]
fn connecting_the_app_preserves_idle_session_activity(cx: &mut TestAppContext) {
    let Some(running) = start_config("terminals-idle-journey", false, false, false, true) else {
        return;
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let read = || {
        let args = ["list", "--json", "--node", &running.url].map(str::to_owned);
        let mut output = Vec::new();
        assert_eq!(
            runtime
                .block_on(parvi_node_kit::cli::run("session", &args, &mut output))
                .unwrap(),
            0
        );
        serde_json::from_slice::<serde_json::Value>(&output).unwrap()
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    let baseline = loop {
        let value = read();
        let sessions = value["sessions"].as_array().unwrap();
        if sessions.len() == 2
            && sessions
                .iter()
                .all(|s| s["active"] == false && s["last_output"].is_number())
        {
            break sessions
                .iter()
                .map(|s| {
                    (
                        s["name"].as_str().unwrap().to_string(),
                        s["last_output"].clone(),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>();
        }
        assert!(
            Instant::now() < deadline,
            "sessions did not become idle: {value}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    // Make a changed activity timestamp observable even though the CLI
    // reports whole seconds.
    let newest = baseline
        .values()
        .filter_map(serde_json::Value::as_u64)
        .max()
        .unwrap();
    while std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        <= newest + 1
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    let config = format!(
        "[terminals]\nsocket = {:?}\nlaunch = {:?}\n",
        running.socket,
        running.fixture.join("no-launch.conf")
    );
    let second = runtime.block_on(Node::start(AREAS, &config)).unwrap();
    // Nobody looks at the second node, so it lists when asked.
    let second_lists = |what: &str, check: &dyn Fn(&[parvi_term::engine::SessionInfo]) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let sessions = runtime.block_on(second.sessions().listed()).unwrap();
            if check(&sessions) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "second node: {what}: {sessions:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let seconds = |s: &parvi_term::engine::SessionInfo| {
        s.activity
            .last_output
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|age| age.as_secs())
    };
    second_lists(
        "lists both sessions with their old output times",
        &|sessions| {
            sessions.len() == 2
                && sessions
                    .iter()
                    .all(|s| seconds(s) == baseline[&s.name].as_u64())
        },
    );
    let (_shell, cx, settings) = open_with_alpha(&running, "idle-journey", cx);
    // Exercise a resize even when another follower makes automatic claim wait.
    cx.simulate_keystrokes("alt-c");
    let page = PAGE.with(|p| p.borrow().clone()).unwrap();
    until(cx, "app follows the session", |cx| {
        page.read_with(cx, |p, _| {
            p.fleet
                .all
                .iter()
                .any(|s| s.name == "alpha" && s.seat == Some(parvi_terminals_contract::Seat::Mine))
        })
    });
    let after = read();
    for session in after["sessions"].as_array().unwrap() {
        let name = session["name"].as_str().unwrap();
        assert_eq!(
            session["last_output"], baseline[name],
            "{name} changed idle time on connect: {after}"
        );
        assert_eq!(
            session["active"], false,
            "{name} became active on connect: {after}"
        );
    }
    for session in second.sessions().list() {
        assert_eq!(
            session
                .activity
                .last_output
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            baseline[&session.name].as_u64().unwrap(),
            "second node changed {} on app connection",
            session.name
        );
    }
    tmux(
        &running,
        &[
            "send-keys",
            "-t",
            "alpha",
            "printf 'real-output\\n'",
            "Enter",
        ],
    );
    until(cx, "actual output marks the session active", |cx| {
        page.read_with(cx, |p, _| {
            p.fleet
                .all
                .iter()
                .any(|s| s.name == "alpha" && s.active == Some(true))
        })
    });
    let after_output = read();
    let alpha = after_output["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "alpha")
        .unwrap();
    assert!(alpha["last_output"].as_u64().unwrap() > baseline["alpha"].as_u64().unwrap());
    second_lists("observes actual output", &|sessions| {
        let of = |name: &str| sessions.iter().find(|s| s.name == name).and_then(seconds);
        of("alpha") > baseline["alpha"].as_u64() && of("beta") == baseline["beta"].as_u64()
    });
    runtime.block_on(second.stop());
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}
