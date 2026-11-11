//! A failed checkout fetch must not make installed harnesses look broken.
#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::link::client::LinkState;
use parvi_testkit::{fresh_dir, node::Node, sessions};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Running {
    url: String,
    node: String,
    home: PathBuf,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
fn executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
fn start() -> Running {
    let home = fresh_dir("parvi-agents-fetch-page");
    let bin = home.join("bin");
    let kit = home.join("kit");
    let skill = kit.join("skills/sample");
    let links = home.join(".agents/skills");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(kit.join(".git")).unwrap();
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::create_dir_all(&links).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: sample\ndescription: Sample\n---\n",
    )
    .unwrap();
    symlink(&skill, links.join("sample")).unwrap();
    for name in ["claude", "codex", "pi"] {
        executable(&bin.join(name), "#!/bin/sh\necho '1.2.3'\n");
    }
    executable(
        &bin.join("git"),
        "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = fetch ]; then\n    if [ \"$GIT_TERMINAL_PROMPT\" != 0 ]; then echo 'unexpected credential prompt' >&2; exit 2; fi\n    echo 'fatal: could not read Username: terminal prompts disabled' >&2\n    exit 1\n  fi\ndone\nexit 0\n",
    );
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let worker_home = home.clone();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let config = format!(
                    "[agents]\nhome = {:?}\npath = [{:?}, \"/usr/bin\", \"/bin\"]\n",
                    worker_home,
                    worker_home.join("bin")
                );
                let node = Node::start(&[sessions::LISTING, parvi_agents::LISTING], &config)
                    .await
                    .unwrap();
                ready.send((node.url(), node.id())).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                node.stop().await;
            })
    });
    let (url, node) = wait.recv_timeout(Duration::from_secs(20)).unwrap();
    Running {
        url,
        node,
        home,
        stop,
        thread: Some(thread),
    }
}
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut ok: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
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
fn failed_fetch_is_reported_once_without_marking_available_harnesses_broken(
    cx: &mut TestAppContext,
) {
    let running = start();
    let settings = fresh_dir("parvi-agents-fetch-settings");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let path = settings.join("app.json");
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_agents_app::build],
            Some(url),
            Some(path),
            window,
            cx,
        )
    });
    until(cx, "node link", |cx| {
        shell.read_with(cx, |shell, cx| {
            shell.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    let failure = Box::leak(format!("agents-sync-failure-{}-kit", running.node).into_boxed_str());
    until(cx, "fetch failure", |cx| cx.debug_bounds(failure).is_some());
    let message = "agents-sync-message-Sync failed: fatal: could not read Username: terminal prompts disabled";
    assert!(cx.debug_bounds(message).is_some());
    for name in ["claude", "codex", "pi"] {
        let available = Box::leak(format!("agents-tool-state-{name}-available").into_boxed_str());
        let broken = Box::leak(format!("agents-tool-state-{name}-broken").into_boxed_str());
        assert!(
            cx.debug_bounds(available).is_some(),
            "{name} should be available"
        );
        assert!(cx.debug_bounds(broken).is_none());
    }
    // The same checkout is linked to multiple harnesses, but its failure is
    // presented once and stays visible when the operator opens the checkout.
    let checkout = cx.debug_bounds("agents-checkout-kit").unwrap().center();
    cx.simulate_click(checkout, Modifiers::none());
    until(cx, "checkout detail", |cx| {
        cx.debug_bounds(failure).is_some()
    });
    assert!(cx.debug_bounds(message).is_some());
    let _ = std::fs::remove_dir_all(settings);
}
