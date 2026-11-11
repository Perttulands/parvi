//! Reporting from another page against an isolated Beads store.
#![cfg(unix)]
use gpui::{
    App, Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, div, prelude::*,
};
use parvi_app_kit::{Host, Mounted, Page, mount};
use parvi_testkit::node::Node;
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Toy;
impl Page for Toy {
    const ID: &'static str = "toy";
    const TITLE: &'static str = "Toy";
}
impl Render for Toy {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("Toy")
    }
}
fn toy(host: Host, window: &mut Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, |_, _, _| Toy)
}

fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut ready: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if ready(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn bd(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("bd")
        .args(args)
        .current_dir(dir)
        .env_remove("BEADS_DIR")
        .env_remove("BEADS_DB")
        .env_remove("BD_DB")
        .output()
        .unwrap()
}

#[gpui::test]
fn report_files_from_the_current_page_and_escape_discards(cx: &mut TestAppContext) {
    let dir = parvi_testkit::fresh_dir("parvi-report-store");
    let init = bd(
        &dir,
        &[
            "init",
            "--prefix",
            "reporttest",
            "--skip-agents",
            "--skip-hooks",
            "--non-interactive",
        ],
    );
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let store = dir.clone();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let config = format!(
                "[work]\nprojects = [{:?}]\n",
                format!("beads {}", store.display())
            );
            let node = Node::start(&[parvi_work::LISTING], &config).await.unwrap();
            ready.send(node.url()).unwrap();
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            node.stop().await;
        });
    });
    let url = wait.recv().unwrap();
    let settings = parvi_testkit::fresh_dir("parvi-report-settings");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[toy, parvi_work_app::build],
            Some(url),
            Some(settings.join("app.json")),
            window,
            cx,
        )
    });
    until(cx, "link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == parvi_app_kit::link::client::LinkState::Open
        })
    });
    let host = shell.read_with(cx, |s, _| Host::new(s.hub()));
    let sessions = cx
        .update(|_, cx| host.model::<parvi_terminals_contract::SessionList>(cx))
        .unwrap();
    sessions.update(cx, |s, cx| {
        s.focused = Some(parvi_terminals_contract::FocusedSession {
            node: "node".into(),
            user: "user".into(),
            name: "alpha".into(),
        });
        cx.notify();
    });
    cx.simulate_keystrokes("alt-r");
    until(cx, "context", |cx| {
        cx.debug_bounds("report-context-Context: alpha").is_some()
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    until(cx, "project", |cx| {
        cx.debug_bounds("report-project-0").is_some()
    });
    cx.simulate_input("Title");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Body");
    cx.simulate_keystrokes("ctrl-enter");
    until(cx, "filed", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub()
                .read(cx)
                .status
                .as_ref()
                .is_some_and(|s| s.text.starts_with("Filed reporttest-"))
        })
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    let list = bd(&dir, &["--json", "list", "--limit", "0"]);
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let items: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(items[0]["title"], "Title");
    assert_eq!(items[0]["description"], "Body\n\nContext: alpha");
    cx.simulate_keystrokes("alt-r");
    until(cx, "report reopened", |cx| {
        cx.debug_bounds("report-box").is_some()
    });
    cx.simulate_input("Discard me");
    cx.simulate_keystrokes("escape");
    until(cx, "report closed", |cx| {
        cx.debug_bounds("report-box").is_none()
    });
    let list = bd(&dir, &["--json", "list", "--limit", "0"]);
    let items: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1);
    stop.send(()).unwrap();
    thread.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
