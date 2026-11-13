//! Both host pages render data from a testkit node.
use gpui::{TestAppContext, VisualTestContext};
use parvi_app_kit::link::client::LinkState;
use parvi_testkit::node::Node;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

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
            let _ = t.join();
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
                let node = Node::start(&[parvi_host::LISTING], "").await.unwrap();
                // A read-only fixture with an invalid user is never sent to a real manager.
                std::fs::write(node.dir().join("services.json"), serde_json::json!({"services": [
                {"id":"fixture", "name":"Fixture service", "readOnly":true,
                 "components":[{"kind":"systemd-user", "user":"\u{0}", "unit":"fixture.service"}]}
            ]}).to_string()).unwrap();
                ready.send((node.url(), node.id())).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                node.stop().await;
            });
    });
    let (url, node) = wait.recv().unwrap();
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
fn readings_and_services_render_node_data(cx: &mut TestAppContext) {
    let running = start();
    let dir = parvi_testkit::fresh_dir("parvi-host-page");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (url, settings) = (running.url.clone(), dir.join("app.json"));
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_host_app::build_host, parvi_host_app::build_services],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    until(cx, "the node link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    let metric = Box::leak(format!("host-row-{}-cpu", running.node).into_boxed_str());
    until(cx, "a host reading", |cx| cx.debug_bounds(metric).is_some());
    cx.update(|window, cx| shell.update(cx, |s, cx| s.show(1, window, cx)));
    let row = Box::leak(format!("svc-row-{}-fixture", running.node).into_boxed_str());
    until(cx, "the fixture service", |cx| {
        cx.debug_bounds(row).is_some()
    });
    assert!(cx.debug_bounds("services-refresh").is_some());
    until(cx, "the only service, chosen", |cx| {
        cx.debug_bounds("pane-title-Fixture service").is_some()
    });
    for verb in ["start", "stop", "restart"] {
        let control = Box::leak(format!("svc-{verb}-{}-fixture", running.node).into_boxed_str());
        assert!(cx.debug_bounds(control).is_none(), "read-only: no {verb}");
    }
    let _ = std::fs::remove_dir_all(dir);
}
