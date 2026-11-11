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
                let original = include_str!("fixtures/solo-lab.mission.toml");
                let (prefix, rest) = original.split_once("[[formation]]").unwrap();
                let (formation, suffix) = rest.split_once("[[end]]").unwrap();
                let mut mission = prefix.to_owned();
                for n in 1..=5 {
                    let formation = formation.replace("fmn_work", &format!("fmn_{n}"))
                        .replace("slot_work", &format!("slot_{n}_opaque"))
                        .replace("title = \"Work\"", &format!("title = \"Step {n}\""));
                    mission.push_str("[[formation]]"); mission.push_str(&formation);
                }
                mission.push_str("[[end]]");
                mission.push_str(&suffix.replace("fmn_work", "fmn_1"));
                mission = mission.replace("from = \"fmn_1:port_out\"", "from = \"fmn_5:port_out\"");
                for n in 1..4 {
                    mission.push_str(&format!("\n[[connection]]\nid = \"edge_{n}\"\nfrom = \"fmn_{n}:port_out\"\nto = \"fmn_{}:port_in\"\n", n + 1));
                }
                std::fs::write(folder.join("hello.mission.toml"), mission).unwrap();
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
fn five_step_canvas_can_pan_zoom_and_connect_visible_ports(cx: &mut TestAppContext) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-five-canvas-{}", std::process::id()))
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
    cx.simulate_resize(gpui::size(px(1600.), px(1000.)));
    until(cx, "link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    let click = |cx: &mut VisualTestContext, id: &str| {
        let id: &'static str = Box::leak(id.to_string().into_boxed_str());
        let at = cx.debug_bounds(id).expect(id).center();
        cx.simulate_click(at, Modifiers::none());
    };
    let mission = format!("mission-{}-hello", running.id);
    until(cx, "mission", |cx| {
        cx.debug_bounds(Box::leak(mission.clone().into_boxed_str()))
            .is_some()
    });
    click(cx, &mission);
    until(cx, "mode", |cx| {
        cx.debug_bounds("mission-mode-canvas").is_some()
    });
    click(cx, "mission-mode-canvas");
    until(cx, "five step canvas", |cx| {
        cx.debug_bounds("mission-card-fmn_5").is_some()
    });
    click(cx, "canvas-fit");
    cx.run_until_parked();
    let bounds = cx.debug_bounds("mission-canvas").unwrap();
    for n in 1..=5 {
        let id = Box::leak(format!("mission-card-fmn_{n}").into_boxed_str());
        let card = cx.debug_bounds(id).unwrap();
        assert!(
            card.center().x >= bounds.origin.x
                && card.center().x < bounds.origin.x + bounds.size.width,
            "card {n}: {card:?}, canvas {bounds:?}"
        );
    }
    assert!(
        cx.debug_bounds("canvas-seat-Worker-Builder-Codex")
            .is_some()
    );
    assert!(
        cx.debug_bounds("canvas-seat-\"Worker\" (slot_1_opaque)-Builder-Codex")
            .is_none()
    );
    let before = cx.debug_bounds("mission-card-fmn_1").unwrap();
    let at = point(bounds.origin.x + px(8.), bounds.origin.y + px(20.));
    let moved = point(at.x + px(30.), at.y + px(15.));
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(moved, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(moved, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("mission-card-fmn_1").unwrap().origin.x > before.origin.x);
    click(cx, "canvas-fit");
    cx.run_until_parked();
    let before = cx.debug_bounds("mission-card-fmn_1").unwrap();
    cx.simulate_event(gpui::ScrollEvent {
        position: bounds.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-35.))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
        source: gpui::ScrollSource::Pointer,
    });
    cx.run_until_parked();
    assert_ne!(
        cx.debug_bounds("mission-card-fmn_1").unwrap().origin.y,
        before.origin.y
    );
    click(cx, "canvas-fit");
    cx.run_until_parked();
    let from = cx
        .debug_bounds("mission-port-fmn_4:port_out")
        .unwrap()
        .center();
    let to = cx
        .debug_bounds("mission-port-fmn_5:port_in")
        .unwrap()
        .center();
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
    until(cx, "new step connection", |_| {
        rt.block_on(client.request(http::Method::GET, "/api/missions/hello", None, None))
            .unwrap()
            .0["mission"]["connections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["from"] == "fmn_4:port_out" && c["to"] == "fmn_5:port_in")
    });
}
