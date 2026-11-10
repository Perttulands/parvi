//! The Brain rail draws collections read from a testkit node.
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::link::client::LinkState;
use parvi_testkit::{fresh_dir, node::Node};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

#[gpui::test]
fn page_renders_a_collection_from_a_node(cx: &mut TestAppContext) {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let corpus = fresh_dir("brain-page-corpus");
            std::fs::create_dir(corpus.join("notes")).unwrap();
            std::fs::write(corpus.join("notes/page.md"), "# Test page\n").unwrap();
            let config = format!("[brain]\ncorpus = {:?}\n", corpus.to_str().unwrap());
            let node = Node::start(&[parvi_brain::LISTING], &config).await.unwrap();
            ready.send(node.url()).unwrap();
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            node.stop().await;
            std::fs::remove_dir_all(corpus).unwrap();
        });
    });
    let url = wait.recv().unwrap();
    let settings = fresh_dir("brain-page-settings");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (shell, cx) = cx.add_window_view(|window, cx| {
        parvi_app_shell::start(
            &[parvi_brain_app::build],
            Some(url),
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
    until(cx, "Brain collection", |cx| {
        cx.debug_bounds("brain-collection-0").is_some()
    });
    let collection = cx.debug_bounds("brain-collection-0").unwrap().center();
    cx.simulate_click(collection, Modifiers::none());
    until(cx, "collection listing", |cx| {
        cx.debug_bounds("brain-shelf-0").is_some()
    });
    let page = cx.debug_bounds("brain-shelf-0").unwrap().center();
    cx.simulate_click(page, Modifiers::none());
    until(cx, "page reader", |cx| {
        cx.debug_bounds("brain-page-title").is_some()
    });
    stop.send(()).unwrap();
    thread.join().unwrap();
    std::fs::remove_dir_all(settings).unwrap();
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
