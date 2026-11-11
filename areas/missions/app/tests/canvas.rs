#![cfg(unix)]
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
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
                let mission = include_str!("fixtures/solo-lab.mission.toml").replace("[[formation]]", "[[inputCard]]\nid = \"inp_other\"\ntitle = \"Other input\"\ngoal = \"Alternative\"\ninputs = [{ name = \"second\", kind = \"text\", required = false }]\n\n[[formation]]");
                std::fs::write(
                    folder.join("hello.mission.toml"),
                    mission,
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
fn canvas_ports_drag_move_zoom_and_undo(cx: &mut TestAppContext) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-canvas-{}", std::process::id()))
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
    until(cx, "link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    let mission = format!("mission-{}-hello", running.id);
    until(cx, "mission", |cx| {
        cx.debug_bounds(Box::leak(mission.clone().into_boxed_str()))
            .is_some()
    });
    let click = |cx: &mut VisualTestContext, id: &str| {
        let at = cx
            .debug_bounds(Box::leak(id.to_string().into_boxed_str()))
            .expect(id)
            .center();
        cx.simulate_click(at, Modifiers::none());
    };
    click(cx, &mission);
    until(cx, "mode controls", |cx| {
        cx.debug_bounds("mission-mode-canvas").is_some()
    });
    click(cx, "mission-mode-canvas");
    until(cx, "canvas", |cx| {
        cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    click(cx, "canvas-fit");
    cx.run_until_parked();
    let play = cx.debug_bounds("canvas-play-inp_other").unwrap().center();
    let canvas_bounds = cx.debug_bounds("mission-canvas").unwrap();
    assert!(
        play.x >= canvas_bounds.origin.x
            && play.x <= canvas_bounds.origin.x + canvas_bounds.size.width
            && play.y >= canvas_bounds.origin.y
            && play.y <= canvas_bounds.origin.y + canvas_bounds.size.height,
        "input play must be within canvas: {play:?}, {canvas_bounds:?}"
    );
    click(cx, "canvas-play-inp_other");
    until(cx, "second input play opens its form", |cx| {
        cx.debug_bounds("run-input-second").is_some()
    });
    assert!(cx.debug_bounds("run-input-brief").is_none());
    click(cx, "run-cancel");
    until(cx, "canvas reopened", |cx| {
        cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    click(cx, "canvas-fit");
    cx.run_until_parked();
    let before = cx.debug_bounds("mission-card-fmn_work").unwrap();
    let output = cx.debug_bounds("mission-port-fmn_work:port_out").unwrap();
    assert!(
        output.center().x >= before.origin.x
            && output.center().x <= before.origin.x + before.size.width + px(12.)
    );
    let start = before.center();
    let end = point(start.x + px(45.), start.y + px(25.));
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (layout, _) = rt
            .block_on(client.request(http::Method::GET, "/api/missions/hello/layout", None, None))
            .unwrap();
        if layout["layout"]["nodes"]
            .as_array()
            .is_some_and(|a| a.iter().any(|n| n["id"] == "fmn_work"))
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    until(cx, "move acknowledged", |cx| {
        cx.debug_bounds("undo").is_some()
    });
    let moved = cx.debug_bounds("mission-card-fmn_work").unwrap();
    assert!(moved.origin.x > before.origin.x);
    click(cx, "undo");
    until(cx, "undo move", |cx| {
        cx.debug_bounds("mission-card-fmn_work")
            .is_some_and(|b| (f32::from(b.origin.x) - f32::from(before.origin.x)).abs() < 2.)
    });
    let canvas = cx.debug_bounds("mission-canvas").unwrap();
    let background = point(canvas.origin.x + px(10.), canvas.origin.y + px(20.));
    let pan_end = point(background.x + px(20.), background.y + px(10.));
    let before_pan = cx.debug_bounds("mission-card-fmn_work").unwrap();
    cx.simulate_mouse_down(background, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(pan_end, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(pan_end, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("mission-card-fmn_work").unwrap().origin.x > before_pan.origin.x);
    click(cx, "canvas-fit");
    cx.run_until_parked();
    let old_width = cx.debug_bounds("mission-card-fmn_work").unwrap().size.width;
    click(cx, "canvas-in");
    cx.run_until_parked();
    assert!(cx.debug_bounds("mission-card-fmn_work").unwrap().size.width > old_width);
    click(cx, "canvas-fit");
    cx.run_until_parked();
    let to = cx
        .debug_bounds("mission-port-end_done:in")
        .unwrap()
        .center();
    // The existing input can be lifted and dropped on the background, then reconnected.
    cx.simulate_mouse_down(to, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(to.x - px(20.), to.y + px(80.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        point(to.x - px(20.), to.y + px(80.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (doc, _) = rt
            .block_on(client.request(http::Method::GET, "/api/missions/hello", None, None))
            .unwrap();
        if !doc["mission"]["connections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["to"] == "end_done:in")
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    until(cx, "unwire acknowledged and validation refreshed", |cx| {
        cx.debug_bounds("undo").is_some()
            && cx.debug_bounds("mission-validation-complete").is_some()
    });
    click(cx, "canvas-fit");
    cx.run_until_parked();
    let from = cx
        .debug_bounds("mission-port-fmn_work:port_out")
        .unwrap()
        .center();
    let to = cx
        .debug_bounds("mission-port-end_done:in")
        .unwrap()
        .center();
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (doc, _) = rt
            .block_on(client.request(http::Method::GET, "/api/missions/hello", None, None))
            .unwrap();
        if doc["mission"]["connections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["to"] == "end_done:in")
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    until(cx, "wire acknowledged and validation refreshed", |cx| {
        cx.debug_bounds("undo").is_some()
            && cx.debug_bounds("mission-validation-complete").is_some()
    });
    let canvas = cx.debug_bounds("mission-canvas").unwrap();
    let at = point(canvas.origin.x + px(20.), canvas.origin.y + px(20.));
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    until(cx, "context menu", |cx| {
        cx.debug_bounds("canvas-new-gate").is_some()
    });
    cx.simulate_keystrokes("escape");
    until(cx, "menu closed with Escape", |cx| {
        cx.debug_bounds("canvas-new-gate").is_none()
    });
    let canvas = cx.debug_bounds("mission-canvas").unwrap();
    let at = point(canvas.origin.x + px(20.), canvas.origin.y + px(20.));
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    until(cx, "context menu reopened", |cx| {
        cx.debug_bounds("canvas-new-gate").is_some()
    });
    click(cx, "canvas-new-gate");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (doc, _) = rt
            .block_on(client.request(http::Method::GET, "/api/missions/hello", None, None))
            .unwrap();
        if !doc["mission"]["gates"].as_array().unwrap().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    until(cx, "new gate acknowledged", |cx| {
        cx.debug_bounds("undo").is_some()
    });
}
