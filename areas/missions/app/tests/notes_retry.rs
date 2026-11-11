#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_node_kit::Listing;
use parvi_testkit::{node::Node, sessions};
use std::{
    path::PathBuf,
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
    bad: PathBuf,
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
                let root = node.dir().join("missions");
                std::fs::create_dir_all(root.join("missions")).unwrap();
                std::fs::create_dir_all(root.join("notes")).unwrap();
                std::fs::write(
                    root.join("missions/hello.mission.toml"),
                    include_str!("fixtures/solo-lab.mission.toml"),
                )
                .unwrap();
                let bad = root.join("notes/hello.notes.toml");
                std::fs::write(&bad, "invalid = [").unwrap();
                ready.send((node.url(), node.id(), bad)).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                node.stop().await;
            })
    });
    let (url, node, bad) = wait.recv_timeout(Duration::from_secs(20)).unwrap();
    Running {
        url,
        node,
        bad,
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
fn has(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.debug_bounds(Box::leak(id.to_owned().into_boxed_str()))
        .is_some()
}
fn click(cx: &mut VisualTestContext, id: &str) {
    let b = cx
        .debug_bounds(Box::leak(id.to_owned().into_boxed_str()))
        .expect(id);
    cx.simulate_click(b.center(), Modifiers::none());
}
#[gpui::test]
fn failed_note_read_can_be_retried(cx: &mut TestAppContext) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-note-retry-{}", std::process::id()))
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
    let mission = format!("mission-{}-hello", running.node);
    until(cx, "mission", |cx| has(cx, &mission));
    click(cx, &mission);
    until(cx, "canvas mode", |cx| has(cx, "mission-mode-canvas"));
    click(cx, "mission-mode-canvas");
    until(cx, "notes read failure", |cx| {
        has(cx, "canvas-notes-error") && has(cx, "canvas-notes-retry")
    });
    std::fs::remove_file(&running.bad).unwrap();
    click(cx, "canvas-notes-retry");
    until(cx, "notes recovered", |cx| {
        !has(cx, "canvas-notes-error") && has(cx, "canvas-note-pin-fmn_work")
    });
}
