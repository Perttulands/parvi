//! The Files page draws a node's real listing.
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_testkit::node::Node;
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
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}
fn start(root: std::path::PathBuf) -> Running {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let config = format!(
                "[files]\nroots = [{}]\n",
                serde_json::to_string(root.to_str().unwrap()).unwrap()
            );
            let node = Node::start(&[parvi_files::LISTING], &config).await.unwrap();
            ready.send(node.url()).unwrap();
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            node.stop().await;
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
fn page_renders_node_data(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-page");
    std::fs::write(root.join("note.txt"), "hello").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let settings = root.join("app.json");
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_files_app::build],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    until(cx, "Files page", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    until(cx, "node listing", |cx| {
        cx.debug_bounds("file-row-0-note.txt").is_some()
    });
    let at = cx.debug_bounds("file-row-0-note.txt").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    until(cx, "file viewer", |cx| {
        cx.debug_bounds("files-edit").is_some()
    });
    let edit = cx.debug_bounds("files-edit").unwrap().center();
    cx.simulate_click(edit, Modifiers::none());
    until(cx, "editing", |cx| cx.debug_bounds("files-save").is_some());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("edited");
    cx.simulate_keystrokes("ctrl-s");
    until(cx, "saved text", |_| {
        std::fs::read(root.join("note.txt")).ok().as_deref() == Some(b"edited")
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}
