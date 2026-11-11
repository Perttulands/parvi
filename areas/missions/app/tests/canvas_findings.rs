#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::link::client::LinkState;
use parvi_node_kit::{Listing, NodeClient};
use parvi_testkit::{node::Node, sessions};
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
                let clean = include_str!("fixtures/validation-clean.mission.toml");
                std::fs::write(folder.join("clean.mission.toml"), clean).unwrap();
                let invalid = clean.replace("msn_clean", "msn_findings")
                    .replace("slug = \"clean\"", "slug = \"findings\"")
                    .replace("Clean delivery", "Delivery with findings")
                    .replace("harness = \"openai-codex\"", "harness = \"\"")
                    .replace("kinds = [\"human\"]", "kinds = [\"code\"]")
                    + "\n[[connection]]\nid = \"edge_duplicate\"\nfrom = \"inp_start:out\"\nto = \"gate_review:in\"\n[[connection]]\nid = \"edge_dangling\"\nfrom = \"step_work:out\"\nto = \"missing:in\"\n";
                std::fs::write(folder.join("findings.mission.toml"), invalid).unwrap();
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
fn canvas_shows_exact_findings_on_cards_and_edges_and_clears_them(cx: &mut TestAppContext) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-canvas-findings-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(gpui::px(1600.), gpui::px(1000.)));
    until(cx, "link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    let invalid = Box::leak(format!("mission-{}-findings", running.id).into_boxed_str());
    until(cx, "findings mission", |cx| {
        cx.debug_bounds(invalid).is_some()
    });
    let position = cx.debug_bounds(invalid).unwrap().center();
    cx.simulate_click(position, Modifiers::none());
    until(cx, "mode", |cx| {
        cx.debug_bounds("mission-mode-canvas").is_some()
    });
    let position = cx.debug_bounds("mission-mode-canvas").unwrap().center();
    cx.simulate_click(position, Modifiers::none());
    until(cx, "step, gate and edge marks", |cx| {
        cx.debug_bounds("validation-card-step_work").is_some()
            && cx.debug_bounds("validation-card-gate_review").is_some()
            && cx.debug_bounds("validation-edge-edge_duplicate").is_some()
            && cx.debug_bounds("validation-edge-edge_dangling").is_some()
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
    let (report, _) = rt
        .block_on(client.request(
            http::Method::GET,
            "/api/missions/findings/validation",
            None,
            None,
        ))
        .unwrap();
    for id in [
        "step_work",
        "gate_review",
        "edge_duplicate",
        "edge_dangling",
    ] {
        let finding = report["errors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["nodeId"] == id)
            .unwrap();
        let selector = Box::leak(
            format!(
                "validation-message-{id}-{}",
                finding["message"].as_str().unwrap()
            )
            .into_boxed_str(),
        );
        assert!(
            cx.debug_bounds(selector).is_some(),
            "the exact validation message must be visible for {id}"
        );
    }
    let clean = Box::leak(format!("mission-{}-clean", running.id).into_boxed_str());
    until(cx, "clean listing", |cx| cx.debug_bounds(clean).is_some());
    let position = cx.debug_bounds(clean).unwrap().center();
    cx.simulate_click(position, Modifiers::none());
    until(cx, "clean validation", |cx| {
        cx.debug_bounds("mission-validation-complete").is_some()
            && cx.debug_bounds("mission-card-step_work").is_some()
    });
    let (report, _) = rt
        .block_on(client.request(
            http::Method::GET,
            "/api/missions/clean/validation",
            None,
            None,
        ))
        .unwrap();
    assert!(report["errors"].as_array().unwrap().is_empty());
    assert!(report["warnings"].as_array().unwrap().is_empty());
    assert!(cx.debug_bounds("canvas-findings-list").is_none());
    assert!(cx.debug_bounds("validation-card-step_work").is_none());
    assert!(cx.debug_bounds("validation-card-gate_review").is_none());
    assert!(cx.debug_bounds("validation-edge-edge_duplicate").is_none());
    assert!(cx.debug_bounds("validation-edge-edge_dangling").is_none());
}
