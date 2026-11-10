//! The Terminals page in the app against a real node (terminals over a
//! scratch tmux, started through the testkit): a session attached to a
//! window takes what is typed and shows its output, and a toy page hands a
//! reference into that session through the Send ask; other pages read the
//! sessions the page publishes and open views beside it. Linux and macOS only;
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
use parvi_node_kit::Build;
use parvi_terminals_app::Terminals;
use parvi_terminals_app::store::sessions::Key;
use parvi_terminals_contract::SessionList;
use parvi_testkit::node::Node;
use parvi_testkit::tmux::Server;

const AREAS: &[(&str, Build)] = &[(parvi_terminals::NAME, parvi_terminals::build)];

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
            let config = format!("[terminals]\nsocket = {:?}\n", tmux.socket_path());
            let node = Node::start(AREAS, &config).await.expect("a node");
            let _ = ready.send(Some((node.url(), node.id())));
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            node.stop().await;
        });
    });
    match wait.recv().expect("the node thread") {
        Some((url, node)) => Some(Running {
            url,
            node,
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
