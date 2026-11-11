#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_node_kit::{Listing, NodeClient};
use parvi_testkit::{node::Node, sessions};
use serde_json::json;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
const AREAS: &[Listing] = &[
    sessions::LISTING,
    parvi_work::LISTING,
    parvi_missions::LISTING,
];
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
            thread.join().unwrap();
        }
    }
}
fn start() -> Running {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let node = Node::start(AREAS, "").await.unwrap();
                ready.send((node.url(), node.id())).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                node.stop().await;
            })
    });
    let (url, node) = wait.recv_timeout(Duration::from_secs(20)).unwrap();
    Running {
        url,
        node,
        stop,
        thread: Some(thread),
    }
}
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut f: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if f(cx) {
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn has(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .is_some()
}
fn click(cx: &mut VisualTestContext, selector: &str) {
    let bounds = cx
        .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .expect(selector);
    cx.simulate_click(bounds.center(), Modifiers::none());
}
#[gpui::test]
fn creates_and_renames_a_mission_with_cancel_and_conflict_guard(cx: &mut TestAppContext) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-manage-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    until(cx, "new mission", |cx| has(cx, "new-mission"));
    click(cx, "new-mission");
    until(cx, "create form", |cx| has(cx, "mission-form-name"));
    cx.simulate_input("Abandoned");
    click(cx, "mission-form-cancel");
    until(cx, "form canceled", |cx| !has(cx, "mission-form"));
    click(cx, "new-mission");
    until(cx, "fresh create form", |cx| has(cx, "mission-form-name"));
    cx.simulate_input("@@@");
    click(cx, "mission-form-submit");
    until(cx, "invalid title", |cx| has(cx, "mission-form-error"));
    click(cx, "mission-form-name");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("Release");
    click(cx, "mission-form-submit");
    until(cx, "created", |cx| !has(cx, "mission-form"));
    let rename = format!("mission-rename-{}-release", running.node);
    until(cx, "rename command", |cx| has(cx, &rename));
    click(cx, &rename);
    until(cx, "rename form", |cx| has(cx, "mission-form-name"));
    cx.simulate_keystrokes("ctrl-a backspace");
    click(cx, "mission-form-submit");
    until(cx, "name required", |cx| has(cx, "mission-form-error"));
    click(cx, "mission-form-name");
    cx.simulate_input("Renamed");
    click(cx, "mission-form-submit");
    until(cx, "renamed", |cx| !has(cx, "mission-form"));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let read = || {
        runtime.block_on(async {
            let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
            client
                .request(http::Method::GET, "/api/missions/release", None, None)
                .await
                .unwrap()
        })
    };
    assert_eq!(read().0["mission"]["title"], "Renamed");
    click(cx, &rename);
    until(cx, "rename again", |cx| has(cx, "mission-form-name"));
    cx.simulate_input("Draft name");
    runtime.block_on(async {
        let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
        let (doc, etag) = client
            .request(http::Method::GET, "/api/missions/release", None, None)
            .await
            .unwrap();
        client
            .request(
                http::Method::PATCH,
                "/api/missions/release",
                Some(json!({"title":"External", "expectedRev":doc["mission"]["rev"]})),
                Some(&etag),
            )
            .await
            .unwrap();
    });
    click(cx, "mission-form-submit");
    until(cx, "stale rename refused", |cx| {
        has(cx, "mission-form-error")
    });
    click(cx, "mission-form-name");
    cx.simulate_keystrokes("ctrl-a ctrl-c");
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("Draft name")
    );
    assert_eq!(read().0["mission"]["title"], "External");
    click(cx, "mission-form-cancel");
    until(cx, "stale form canceled", |cx| !has(cx, "mission-form"));
    click(cx, "new-mission");
    until(cx, "second create form", |cx| has(cx, "mission-form-name"));
    cx.simulate_input("Scratch");
    click(cx, "mission-form-submit");
    until(cx, "second mission created", |cx| !has(cx, "mission-form"));
    let delete = format!("mission-delete-{}-scratch", running.node);
    until(cx, "delete command", |cx| has(cx, &delete));
    click(cx, &delete);
    until(cx, "archive confirmation", |cx| {
        has(cx, "mission-archive-explanation")
    });
    click(cx, "mission-form-cancel");
    until(cx, "archive canceled", |cx| !has(cx, "mission-form"));
    click(cx, &delete);
    until(cx, "archive confirmation reopened", |cx| {
        has(cx, "mission-archive-explanation")
    });
    click(cx, "mission-form-submit");
    until(cx, "mission archived", |cx| {
        !has(cx, "mission-form") && !has(cx, &delete)
    });
    runtime.block_on(async {
        let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
        assert!(
            client
                .request(http::Method::GET, "/api/missions/scratch", None, None)
                .await
                .is_err()
        );
    });
}
