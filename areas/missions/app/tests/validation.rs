#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::link::client::LinkState;
use parvi_node_kit::{Listing, NodeClient};
use parvi_testkit::{node::Node, sessions};
use serde_json::json;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const AREAS: &[Listing] = &[
    sessions::LISTING,
    parvi_work::LISTING,
    parvi_missions::LISTING,
];
struct Running {
    url: String,
    id: String,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
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
                let folder = node.dir().join("missions/missions");
                std::fs::create_dir_all(&folder).unwrap();
                std::fs::write(
                    folder.join("hello.mission.toml"),
                    include_str!("fixtures/solo-lab.mission.toml").replace("[[connection]]\nid = \"edge_start\"\nfrom = \"inp_hello:out\"\nto = \"fmn_work:port_in\"\n\n", ""),
                )
                .unwrap();
                ready.send((node.url(), node.id())).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                node.stop().await;
            });
    });
    let (url, id) = wait.recv().unwrap();
    Running {
        url,
        id,
        stop,
        thread: Some(thread),
    }
}
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut f: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        if f(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[gpui::test]
fn run_is_only_emphasized_for_a_current_runnable_mission(cx: &mut TestAppContext) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-missions-validation-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (url, path) = (running.url.clone(), settings.clone());
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build],
            Some(url),
            Some(path),
            window,
            cx,
        )
    });
    until(cx, "link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    until(cx, "no selected mission", |cx| {
        cx.debug_bounds("new-mission").is_some()
    });
    assert!(
        cx.debug_bounds("mission-run-ready").is_none()
            && cx.debug_bounds("mission-run-quiet").is_none(),
        "without a mission there is nothing to run"
    );
    let selector = Box::leak(format!("mission-{}-hello", running.id).into_boxed_str());
    until(cx, "mission listing", |cx| {
        cx.debug_bounds(selector).is_some()
    });
    let mission = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_click(mission, Modifiers::none());
    until(cx, "unwired mission", |cx| {
        cx.debug_bounds("flow-input-inp_hello").is_some()
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
    let (report, _) = rt
        .block_on(client.request(
            http::Method::GET,
            "/api/missions/hello/validation",
            None,
            None,
        ))
        .unwrap();
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "mission_not_runnable")
    );
    // Wait for the page's independent validation request, then check its
    // styling for the input without an outgoing wire.
    until(cx, "invalid validation", |cx| {
        cx.debug_bounds("mission-validation-complete").is_some()
    });
    assert!(cx.debug_bounds("mission-run-quiet").is_some());
    assert!(cx.debug_bounds("mission-run-ready").is_none());
    let rail = cx.debug_bounds("missions-rail").unwrap();
    assert_eq!(
        rail.size.width,
        gpui::px(parvi_app_kit::theme::size::LABEL_COL)
    );
    until(cx, "flow note editor", |cx| {
        cx.debug_bounds("mission-note-draft").is_some()
    });
    let canvas = cx.debug_bounds("mission-mode-canvas").unwrap().center();
    cx.simulate_click(canvas, Modifiers::none());
    until(cx, "canvas mode", |cx| {
        cx.debug_bounds("mission-canvas").is_some()
    });
    assert!(cx.debug_bounds("mission-note-draft").is_none());
    rt.block_on(async {
        let (doc, etag) = client.request(http::Method::GET, "/api/missions/hello", None, None).await.unwrap();
        client.request(http::Method::PATCH, "/api/missions/hello", Some(json!({"expectedRev":doc["mission"]["rev"], "wireConnection":{"from":"inp_hello:out","to":"fmn_work:port_in"}})), Some(&etag)).await.unwrap();
        let (valid, _) = client.request(http::Method::GET, "/api/missions/hello/validation", None, None).await.unwrap();
        assert_eq!(valid["errors"].as_array().unwrap().len(), 0, "{valid}");
        assert!(!valid["warnings"].as_array().unwrap().iter().any(|w| w["code"] == "mission_not_runnable"));
    });
    until(cx, "wired mission is runnable", |cx| {
        cx.debug_bounds("mission-run-ready").is_some()
    });
    assert!(cx.debug_bounds("mission-run-quiet").is_none());
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}
