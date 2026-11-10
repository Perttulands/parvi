//! The Work page's list, card and handoff against a node with a fixture store.
#![cfg(unix)]

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, Window};
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::{Host, Mounted, mount};
use parvi_node_kit::{Built, Listing, Setup};
use parvi_terminals_app::Terminals;
use parvi_testkit::node::Node;
use parvi_testkit::tmux::Server;
use std::cell::RefCell;
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

static BD: OnceLock<String> = OnceLock::new();
fn build_work(setup: &Setup) -> anyhow::Result<Built> {
    parvi_work::build_with(
        setup,
        parvi_work::Bd::new(BD.get().unwrap()),
        parvi_work::Gh::default(),
    )
}
const WORK: Listing = Listing {
    build: build_work,
    ..parvi_work::LISTING
};
const AREAS: &[Listing] = &[parvi_terminals::LISTING, WORK];
thread_local! { static TERMINALS: RefCell<Option<Entity<Terminals>>> = const { RefCell::new(None) }; }
fn terminals(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
    mount(host, window, cx, |host, window, cx| {
        TERMINALS.with(|p| *p.borrow_mut() = Some(cx.entity()));
        Terminals::new(host, window, cx)
    })
}
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
fn start() -> Option<Running> {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let Some(tmux) = Server::start_in("work-page-journey") else { ready.send(None).unwrap(); return; };
            tmux.new_session("alpha", 100, 30);
            let dir = parvi_testkit::fresh_dir("work-page-store");
            std::fs::create_dir_all(dir.join(".beads")).unwrap();
            let script = dir.join("fixture-bd");
            std::fs::write(&script, "#!/bin/sh\nprintf '%s\\n' '[{\"id\":\"fixture-1\",\"title\":\"Read the fixture\",\"status\":\"open\",\"created_at\":\"2026-11-01T00:00:00Z\",\"updated_at\":\"2026-11-01T00:00:00Z\"}]'\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            BD.set(script.to_string_lossy().into_owned()).unwrap();
            let config = format!("[terminals]\nsocket = {:?}\n[work]\nprojects = [{:?}]\n", tmux.socket_path(), format!("beads {}", dir.display()));
            let node = Node::start(AREAS, &config).await.unwrap();
            ready.send(Some((node.url(), node.id()))).unwrap();
            while stopped.try_recv().is_err() { tokio::time::sleep(Duration::from_millis(50)).await; }
            node.stop().await;
            std::fs::remove_dir_all(dir).unwrap();
        });
    });
    match wait.recv().unwrap() {
        Some((url, node)) => Some(Running {
            url,
            node,
            stop,
            thread: Some(thread),
        }),
        None => {
            thread.join().unwrap();
            assert!(
                std::env::var_os("PARVI_REQUIRE_TMUX").is_none(),
                "tmux is required"
            );
            None
        }
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
fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = cx
        .debug_bounds(Box::leak(selector.to_string().into_boxed_str()))
        .expect(selector)
        .center();
    cx.simulate_click(at, Modifiers::none());
}
fn shown(cx: &mut VisualTestContext) -> String {
    TERMINALS
        .with(|p| p.borrow().clone())
        .unwrap()
        .read_with(cx, |p, _| p.window_text(0))
        .unwrap_or_default()
}
#[gpui::test]
fn opens_a_work_item_and_sends_its_reference_to_a_session(cx: &mut TestAppContext) {
    let Some(running) = start() else {
        return;
    };
    let settings = parvi_testkit::fresh_dir("work-page-settings");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let path = settings.join("app.json");
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[terminals, parvi_work_app::build],
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
    let row = format!("row-{}::alpha", running.node);
    until(cx, "session", |cx| {
        cx.debug_bounds(Box::leak(row.clone().into_boxed_str()))
            .is_some()
    });
    click(cx, &row);
    until(cx, "peek", |cx| cx.debug_bounds("peek-attach").is_some());
    click(cx, "peek-attach");
    until(cx, "terminal", |cx| !shown(cx).trim().is_empty());
    click(cx, "tab-work");
    until(cx, "fixture item", |cx| {
        cx.debug_bounds("work-row-fixture-1").is_some()
    });
    click(cx, "work-row-fixture-1");
    until(cx, "item card", |cx| cx.debug_bounds("work-card").is_some());
    click(cx, "work-send");
    until(cx, "send drawer", |cx| cx.debug_bounds("send-go").is_some());
    click(cx, "send-go");
    until(cx, "reference in session", |cx| {
        shown(cx).contains("bead fixture-1: Read the fixture")
    });
    std::fs::remove_dir_all(settings).unwrap();
}
