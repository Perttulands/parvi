#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_node_kit::{Built, Listing, Setup};
use parvi_testkit::{node::Node, sessions};
use serde_json::json;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

fn lab(setup: &Setup) -> anyhow::Result<Built> {
    parvi_missions::build_with_executor(setup, Some(Box::new(parvi_mission_engine::LabExecutor)))
}
const MISSIONS: Listing = Listing {
    build: lab,
    ..parvi_missions::LISTING
};
struct Running {
    url: String,
    node: String,
    oldest: String,
    latest: String,
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
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
        let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, MISSIONS], "").await.unwrap();
        let dir = node.dir().join("missions/missions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hello.mission.toml"), include_str!("fixtures/solo-lab.mission.toml")).unwrap();
        let mut runs = Vec::new();
        for _ in 0..6 {
            let answer = node.call("mission_run", json!({"mission":"hello","input":"inp_hello","inputs":{"brief":"ship"},"cwd":node.dir()})).await.unwrap();
            let id = answer["runId"].as_str().unwrap().to_owned();
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let state = node.call("run_status", json!({"run":id})).await.unwrap();
                if state["final"] == true || state["status"] == "blocked" {
                    assert_eq!(state["status"], "succeeded", "{state}");
                    runs.push((state["startedAt"].as_str().unwrap().to_owned(), id));
                    break;
                }
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        runs.sort();
        ready.send((node.url(), node.id(), runs.first().unwrap().1.clone(), runs.last().unwrap().1.clone())).unwrap();
        while stopped.try_recv().is_err() { tokio::time::sleep(Duration::from_millis(20)).await; }
        node.stop().await;
    })
    });
    let (url, node, oldest, latest) = wait.recv_timeout(Duration::from_secs(30)).unwrap();
    Running {
        url,
        node,
        oldest,
        latest,
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
fn selector(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .is_some()
}
fn click(cx: &mut VisualTestContext, selector: &str) {
    let b = cx
        .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .expect(selector);
    cx.simulate_click(b.center(), Modifiers::none());
}
#[gpui::test]
fn the_list_opens_the_latest_run_and_reaches_every_run_of_a_mission(cx: &mut TestAppContext) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-nav-{}", std::process::id()))
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
    let title = format!("mission-{}-hello", running.node);
    let oldest = format!("run-row-{}", running.oldest);
    let latest = format!("run-row-{}", running.latest);
    // Nothing chosen: the page shows the newest finished run without a click.
    until(cx, "latest run shown", |cx| {
        selector(cx, &title)
            && selector(cx, &latest)
            && selector(cx, &oldest)
            && selector(cx, &format!("run-detail-{}", running.latest))
            && selector(cx, "run-finished-succeeded")
    });
    click(cx, &title);
    until(cx, "mission opened by its name", |cx| {
        selector(cx, "flow-step-fmn_work")
    });
    // The mission lists all its runs; the oldest opens from there.
    let listed = format!("mission-run-{}", running.oldest);
    until(cx, "the mission's runs", |cx| selector(cx, &listed));
    click(cx, &listed);
    until(cx, "older run", |cx| {
        selector(cx, &format!("run-detail-{}", running.oldest))
            && !selector(cx, "flow-step-fmn_work")
    });
    click(cx, "mission-filter");
    cx.simulate_input(&running.oldest);
    until(cx, "filtered run", |cx| {
        selector(cx, &oldest) && !selector(cx, &latest)
    });
}
