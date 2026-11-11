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
use parvi_app_kit::{Ask, Host, Mounted, Page, mount};
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
                    "#!/bin/sh\nprintf 'Options:\\n  -v, --verbose  Verbose output\\n'\n",
                );
                std::fs::write(&launch, format!("harness fake {}\n", binary.display())).unwrap();
            }
            let config = format!(
                "[terminals]\nsocket = {:?}\nlaunch = {:?}\n",
                tmux.socket_path(),
                launch
            );
            let node = Node::start(AREAS, &config).await.expect("a node");
            let _ = ready.send(Some((node.url(), node.id(), tmux.socket_path())));
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            node.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    });
    match wait.recv().expect("the node thread") {
        Some((url, node, socket)) => Some(Running {
            url,
            node,
            socket,
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
        scroll_to_launch(cx, &format!("launcher-scroll-{i}"));
        let button = format!("launch-{i}");
        let selector: &'static str = Box::leak(button.clone().into_boxed_str());
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
    click(cx, "flags-browse-0".into());
    until(cx, "verbose flag", |cx| {
        cx.debug_bounds("flag-0---verbose").is_some()
    });
    click(cx, "flag-0---verbose".into());
    until(cx, "flag in preview", |cx| {
        cx.debug_bounds("flag-line-0---verbose").is_some()
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}
