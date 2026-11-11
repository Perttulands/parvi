//! The operator searches the corpus, opens a result and follows its prose link.
use axum::{Json, routing::post};
use gpui::{Modifiers, TestAppContext, VisualTestContext, point, px};
use parvi_app_kit::link::client::LinkState;
use parvi_testkit::{fresh_dir, node::Node};
use serde_json::{Value, json};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

struct Running {
    url: String,
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
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let corpus = fresh_dir("brain-journey-corpus");
            std::fs::create_dir(corpus.join("notes")).unwrap();
            std::fs::write(corpus.join("notes/first.md"), "# First\n\n[[second|Second]]\n").unwrap();
            std::fs::write(corpus.join("notes/second.md"), "# Second\n\nLinked page.\n").unwrap();
            let citadel = axum::Router::new().route("/v1/library/search", post(|Json(query): Json<Value>| async move {
                let results = if query["query"] == "first" { json!([{ "path": "notes/first.md", "title": "First", "collection": "notes" }]) } else { json!([]) };
                Json(json!({"results": results}))
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, citadel).await.unwrap() });
            let config = format!("[brain]\ncorpus = {:?}\ncitadel = {:?}\n", corpus.to_str().unwrap(), format!("http://{addr}"));
            let node = Node::start(&[parvi_brain::LISTING], &config).await.unwrap();
            ready.send(node.url()).unwrap();
            while stopped.try_recv().is_err() { tokio::time::sleep(Duration::from_millis(20)).await; }
            node.stop().await;
            server.abort();
            std::fs::remove_dir_all(corpus).unwrap();
        });
    });
    Running {
        url: wait.recv().unwrap(),
        stop,
        thread: Some(thread),
    }
}
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut ok: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        if ok(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[gpui::test]
fn search_open_and_follow_a_link_in_the_reader(cx: &mut TestAppContext) {
    let running = start();
    let settings = fresh_dir("brain-journey-settings");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (shell, cx) = cx.add_window_view(|window, cx| {
        parvi_app_shell::start(
            &[parvi_brain_app::build],
            Some(running.url.clone()),
            Some(settings.join("app.json")),
            window,
            cx,
        )
    });
    until(cx, "node link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    until(cx, "search field", |cx| {
        cx.debug_bounds("brain-search").is_some()
    });
    let search = cx.debug_bounds("brain-search").unwrap().center();
    cx.simulate_click(search, Modifiers::none());
    cx.simulate_input("first");
    cx.executor().advance_clock(Duration::from_millis(250));
    until(cx, "search result", |cx| {
        cx.debug_bounds("brain-result-0").is_some()
    });
    let result = cx.debug_bounds("brain-result-0").unwrap().center();
    cx.simulate_click(result, Modifiers::none());
    until(cx, "first page", |cx| {
        cx.debug_bounds("brain-reading-notes/first.md").is_some()
    });
    let prose = cx.debug_bounds("brain-prose").unwrap();
    cx.simulate_click(prose.origin + point(px(8.), px(8.)), Modifiers::none());
    until(cx, "linked page", |cx| {
        cx.debug_bounds("brain-reading-notes/second.md").is_some()
    });
    std::fs::remove_dir_all(settings).unwrap();
}
