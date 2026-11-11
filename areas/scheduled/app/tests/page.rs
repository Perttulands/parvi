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
            tmux.new_session("beta", 100, 30);
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

    // Publish the two scratch tmux sessions as the terminals page does.
    shell.update(cx, |s, cx| {
        let list = SessionList {
            sessions: ["alpha", "beta"]
                .into_iter()
                .map(|name| Session {
                    node: node.to_string(),
                    name: name.into(),
                    ..Session::default()
                })
                .collect(),
        };
        let terminals = Host::new(s.hub()).for_page("terminals", &[]);
        terminals.publish(list, cx).expect("one publisher");
    });
    let new_button = cx.debug_bounds("scheduled-new").unwrap().center();
    cx.simulate_click(new_button, Modifiers::none());
    let alpha: &'static str = Box::leak(format!("session-rail-{node}-:alpha").into_boxed_str());
    let beta: &'static str = Box::leak(format!("session-rail-{node}-:beta").into_boxed_str());
    until(cx, "both sessions beside the form", |cx| {
        cx.debug_bounds(alpha).is_some() && cx.debug_bounds(beta).is_some()
    });
    let at = cx.debug_bounds(alpha).unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    until(cx, "alpha selected", |cx| {
        cx.debug_bounds("chosen-:alpha").is_some()
    });
    let from = cx.debug_bounds(beta).unwrap().center();
    let to = cx.debug_bounds("scheduled-targets").unwrap().center();
    cx.simulate_mouse_down(from, gpui::MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        from + gpui::point(gpui::px(8.), gpui::px(0.)),
        Some(gpui::MouseButton::Left),
        Modifiers::none(),
    );
    cx.simulate_mouse_move(to, Some(gpui::MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, gpui::MouseButton::Left, Modifiers::none());
    until(cx, "beta dropped", |cx| {
        cx.debug_bounds("chosen-:beta").is_some()
    });
    let prompt = cx.debug_bounds("scheduled-prompt").unwrap().center();
    cx.simulate_click(prompt, Modifiers::none());
    cx.simulate_input("echo journey-42");
    let every = cx.debug_bounds("mode-Every").unwrap().center();
    cx.simulate_click(every, Modifiers::none());
    let save = cx.debug_bounds("form-save").unwrap().center();
    cx.simulate_click(save, Modifiers::none());
    until(cx, "the created task notice", |cx| {
        cx.debug_bounds("scheduled-banner-Task created.").is_some()
    });
    let url = running.url.clone();
    let saved = std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let args = ["list", "--node", &url, "--json"].map(String::from);
            let mut output = Vec::new();
            assert_eq!(cli::run("scheduled", &args, &mut output).await.unwrap(), 0);
            let list: serde_json::Value = serde_json::from_slice(&output).unwrap();
            let task = list["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|task| task["name"] == "echo journey-42")
                .expect("created task");
            let id = task["id"].as_str().unwrap();
            let args = ["show", id, "--node", &url, "--json"].map(String::from);
            let mut output = Vec::new();
            assert_eq!(cli::run("scheduled", &args, &mut output).await.unwrap(), 0);
            serde_json::from_slice::<serde_json::Value>(&output).unwrap()
        })
    })
    .join()
    .unwrap();
    let created = &saved["task"];
    assert_eq!(created["prompt"], "echo journey-42");
    assert_eq!(created["schedule"]["type"], "interval");
    let targets = created["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2);
    assert!(targets.iter().any(|t| t["sessionName"] == "alpha"));
    assert!(targets.iter().any(|t| t["sessionName"] == "beta"));
    let click = |cx: &mut VisualTestContext, id: &'static str| {
        let at = cx.debug_bounds(id).expect(id).center();
        cx.simulate_click(at, Modifiers::none());
    };
    click(cx, "task-run");
    until(cx, "run delivery shown", |cx| {
        cx.debug_bounds("scheduled-banner-Sent to 2 sessions.")
            .is_some()
    });
    click(cx, "task-pause-false");
    until(cx, "paused", |cx| {
        cx.debug_bounds("task-pause-true").is_some()
    });
    click(cx, "task-pause-true");
    until(cx, "resumed", |cx| {
        cx.debug_bounds("task-pause-false").is_some()
    });
    click(cx, "task-edit");
    until(cx, "edit form", |cx| {
        cx.debug_bounds("scheduled-prompt").is_some()
    });
    click(cx, "scheduled-prompt");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("echo edited-42");
    click(cx, "form-save");
    until(cx, "updated task", |cx| {
        cx.debug_bounds("scheduled-banner-Task updated.").is_some()
    });
    click(cx, "task-duplicate");
    until(cx, "duplicate form", |cx| {
        cx.debug_bounds("mode-Cron").is_some()
    });
    click(cx, "mode-Cron");
    until(cx, "cron field", |cx| {
        cx.debug_bounds("scheduled-cron").is_some()
    });
    click(cx, "scheduled-cron");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("0 9 * * 1");
    click(cx, "form-save");
    until(cx, "cron task created", |cx| {
        cx.debug_bounds("scheduled-banner-Task created.").is_some()
    });
    let url = running.url.clone();
    let cron = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let args = ["list", "--node", &url, "--json"].map(String::from);
                let mut output = Vec::new();
                assert_eq!(cli::run("scheduled", &args, &mut output).await.unwrap(), 0);
                let listed: serde_json::Value = serde_json::from_slice(&output).unwrap();
                listed["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|t| t["name"] == "echo journey-42 copy")
                    .cloned()
                    .expect("copied cron task")
            })
    })
    .join()
    .unwrap();
    assert_eq!(cron["schedule"]["type"], "cron");
    assert_eq!(cron["schedule"]["expression"], "0 9 * * 1");
    click(cx, "task-delete-false");
    until(cx, "delete confirmation", |cx| {
        cx.debug_bounds("task-delete-true").is_some()
    });
    click(cx, "task-delete-true");
    until(cx, "task deleted", |cx| {
        cx.debug_bounds("scheduled-banner-Task deleted.").is_some()
    });
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}
