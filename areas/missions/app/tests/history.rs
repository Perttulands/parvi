#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext, point, px};
use parvi_app_kit::{Ask, Host};
use parvi_core::NodeId;
use parvi_node_kit::{Listing, NodeClient};
use parvi_testkit::{node::Node, sessions};
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
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}
fn mission() -> String {
    let source = include_str!("fixtures/solo-lab.mission.toml")
        .replace(
            "[[formation.output]]",
            "[[formation.input]]\nid = \"port_second\"\nlabel = \"Second\"\n[[formation.output]]",
        )
        .replace("to = \"end_done:in\"", "to = \"gate_review:in\"");
    format!(
        "{source}\n[[gate]]\nid = \"gate_review\"\ntitle = \"Review\"\nkinds = [\"human\"]\n[[connection]]\nid = \"edge_pass\"\nfrom = \"gate_review:pass\"\nto = \"end_done:in\"\n[[connection]]\nid = \"edge_feedback\"\nfrom = \"gate_review:fail\"\nto = \"fmn_work:port_in\"\n"
    )
}
fn start() -> Running {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let node = Node::start(AREAS, "").await.unwrap();
                let dir = node.dir().join("missions/missions");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("hello.mission.toml"), mission()).unwrap();
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
    mut condition: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if condition(cx) {
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn scroll(cx: &mut VisualTestContext, delta: f32) {
    cx.simulate_event(gpui::ScrollEvent {
        position: point(px(1100.), px(500.)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(delta))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
        source: gpui::ScrollSource::Pointer,
    });
    cx.run_until_parked();
}
fn click(cx: &mut VisualTestContext, selector: &'static str) {
    if selector.starts_with("port-") || selector == "step-goal" {
        for _ in 0..8 {
            let y = f32::from(cx.debug_bounds(selector).expect(selector).center().y);
            if (90. ..820.).contains(&y) {
                break;
            }
            scroll(cx, (500. - y).clamp(-400., 400.));
        }
    }
    let at = cx.debug_bounds(selector).expect(selector).center();
    cx.simulate_click(at, Modifiers::none());
}
#[gpui::test]
fn undo_and_redo_replay_both_wires_of_a_removed_shared_input(cx: &mut TestAppContext) {
    let running = start();
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let settings = std::env::temp_dir()
        .join(format!("parvi-history-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
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
    cx.simulate_resize(gpui::size(px(1440.), px(900.)));
    shell.update(cx, |shell, cx| {
        Host::new(shell.hub())
            .for_page("test", &[])
            .ask(
                Ask::Reveal {
                    what: "mission".into(),
                    id: "hello".into(),
                    node: Some(NodeId(running.node.clone())),
                    part: None,
                    beside: false,
                },
                cx,
            )
            .unwrap()
    });
    until(cx, "mission mode", |cx| {
        cx.debug_bounds("mission-mode-canvas").is_some()
    });
    click(cx, "mission-mode-canvas");
    until(cx, "mission", |cx| {
        cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    click(cx, "mission-card-fmn_work");
    until(cx, "step", |cx| cx.debug_bounds("step-save").is_some());
    scroll(cx, -500.);
    until(cx, "input port", |cx| {
        cx.debug_bounds("port-port_in").is_some()
    });
    click(cx, "port-port_in");
    click(cx, "port-remove");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
    let read = || {
        rt.block_on(client.request(http::Method::GET, "/api/missions/hello", None, None))
            .unwrap()
            .0
    };
    let targets = |doc: &serde_json::Value| {
        doc["mission"]["connections"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|wire| {
                wire["to"]
                    .as_str()
                    .is_some_and(|to| to.starts_with("fmn_work:"))
            })
            .count()
    };
    until(cx, "remove shared input", |_| targets(&read()) == 0);
    until(cx, "undo ready", |cx| cx.debug_bounds("undo").is_some());
    click(cx, "undo");
    until(cx, "undo restores both wires", |_| targets(&read()) == 2);
    until(cx, "redo ready", |cx| cx.debug_bounds("redo").is_some());
    click(cx, "redo");
    until(cx, "redo removes both wires", |_| targets(&read()) == 0);

    // The page shortcuts operate on the same compound history as the buttons.
    cx.simulate_keystrokes("ctrl-z");
    until(cx, "keyboard undo restores both wires", |_| {
        targets(&read()) == 2
    });
    cx.simulate_keystrokes("ctrl-shift-z");
    until(cx, "keyboard redo removes both wires", |_| {
        targets(&read()) == 0
    });
    cx.simulate_keystrokes("ctrl-z");
    until(cx, "second keyboard undo", |_| targets(&read()) == 2);
    cx.simulate_keystrokes("ctrl-y");
    until(cx, "ctrl-y redo", |_| targets(&read()) == 0);

    // Escape must leave an unsaved field in place and preserve its contents.
    until(cx, "step goal", |cx| cx.debug_bounds("step-goal").is_some());
    click(cx, "step-goal");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("Keep this keyboard draft");
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds("step-goal").is_some());
    cx.simulate_keystrokes("ctrl-a ctrl-c");
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("Keep this keyboard draft")
    );
    assert_eq!(targets(&read()), 0);
}
