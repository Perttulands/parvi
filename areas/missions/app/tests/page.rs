#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::link::client::LinkState;
use parvi_node_kit::Listing;
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
                std::fs::write(
                    folder.join("hello.mission.toml"),
                    include_str!("fixtures/solo-lab.mission.toml"),
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
fn opens_a_fixture_mission_on_the_canvas(cx: &mut TestAppContext) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-missions-page-{}", std::process::id()))
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
    let selector = Box::leak(format!("mission-{}-hello", running.id).into_boxed_str());
    until(cx, "mission listing", |cx| {
        cx.debug_bounds(selector).is_some()
    });
    let at = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    until(cx, "step on canvas", |cx| {
        cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    assert!(cx.debug_bounds("mission-card-inp_hello").is_some());
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}
