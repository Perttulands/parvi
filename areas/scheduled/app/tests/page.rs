//! The Scheduled page opens in the app against a real node (terminals and
//! scheduled over a scratch tmux, started through the testkit) and lists a
//! task made through the CLI, and offers the sessions the terminals page
//! publishes. Linux and macOS only; skips without tmux
//! unless `PARVI_REQUIRE_TMUX=1`.
#![cfg(unix)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::Host;
use parvi_app_kit::link::client::LinkState;
use parvi_node_kit::{Listing, cli};
use parvi_terminals_contract::{Session, SessionList};
use parvi_testkit::node::Node;
use parvi_testkit::tmux::{Server, eventually, session_text};
use serde_json::json;

const AREAS: &[Listing] = &[parvi_terminals::LISTING, parvi_scheduled::LISTING];

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

/// A node with a session `alpha` and one task made through the CLI.
fn start() -> Option<Running> {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("a runtime");
        rt.block_on(async move {
            let Some(tmux) = Server::start_in("scheduled-page") else {
                let _ = ready.send(None);
                return;
            };
            tmux.new_session("alpha", 100, 30);
            let config = format!("[terminals]\nsocket = {:?}\n", tmux.socket_path());
            let node = Node::start(AREAS, &config).await.expect("a node");
            let sessions = node.sessions();
            eventually("alpha's pane read", || {
                session_text(sessions.as_ref(), "alpha")?
                    .contains('$')
                    .then_some(())
            })
            .await;
            let args: Vec<String> = [
                "create",
                "--name",
                "nudge",
                "--prompt",
                "echo hi",
                "--sessions",
                "alpha",
                "--every",
                "60",
                "--node",
                &node.url(),
            ]
            .map(String::from)
            .to_vec();
            let code = cli::run("scheduled", &args, &mut Vec::new())
                .await
                .expect("the CLI ran");
            assert_eq!(code, 0, "the CLI made the task");
            let listed = node.call("scheduled_list", json!({})).await.expect("list");
            let id = listed["tasks"][0]["id"]
                .as_str()
                .expect("its id")
                .to_owned();
            let _ = ready.send(Some((node.url(), node.id(), id)));
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            node.stop().await;
        });
    });
    match wait.recv().expect("the node thread") {
        Some((url, node, task)) => Some(Running {
            url,
            node: format!("{node}\n{task}"),
            stop,
            thread: Some(thread),
        }),
        None => {
            let _ = thread.join();
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

#[gpui::test]
fn the_page_lists_a_task_made_through_the_cli(cx: &mut TestAppContext) {
    let Some(running) = start() else {
        assert!(
            std::env::var_os("PARVI_REQUIRE_TMUX").is_none(),
            "PARVI_REQUIRE_TMUX is set but tmux did not start"
        );
        return;
    };
    let (node, task) = running.node.split_once('\n').expect("node and task");
    let selector = format!("task-{node}-{task}");
    let settings = std::env::temp_dir()
        .join(format!("parvi-scheduled-page-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().expect("its folder")).expect("a folder");
    // The node and the link run on their own threads.
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (url, path) = (running.url.clone(), settings.clone());
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_scheduled_app::build],
            Some(url),
            Some(path),
            window,
            cx,
        )
    });
    until(cx, "the link to open", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    until(cx, "the Scheduled page to show", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("scheduled")
    });
    until(cx, "the task to be listed", |cx| {
        cx.debug_bounds(Box::leak(selector.clone().into_boxed_str()))
            .is_some()
    });

    // Its card selects the task, and the detail names it.
    let at = cx
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .expect("its card")
        .center();
    cx.simulate_click(at, Modifiers::none());
    until(cx, "the detail to open", |cx| {
        cx.debug_bounds("task-run").is_some()
    });

    // The sessions a new task may send to are those the terminals page
    // publishes: a session only that list names is offered.
    shell.update(cx, |s, cx| {
        let list = SessionList {
            sessions: vec![Session {
                node: node.to_string(),
                name: "listed-only".into(),
                ..Session::default()
            }],
        };
        let terminals = Host::new(s.hub()).for_page("terminals", &[]);
        terminals.publish(list, cx).expect("one publisher");
    });
    let at = cx
        .debug_bounds("scheduled-new")
        .expect("the New task button")
        .center();
    cx.simulate_click(at, Modifiers::none());
    let pick = format!("pick-{node}-:listed-only");
    until(cx, "the published session to be offered", |cx| {
        cx.debug_bounds(Box::leak(pick.clone().into_boxed_str()))
            .is_some()
    });
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}
