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
    let links = home.join(".agents/skills");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&links).unwrap();
    for name in ["kit", "extras"] {
        let checkout = home.join(name);
        let skill = checkout.join(format!("skills/{name}-skill"));
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}-skill\ndescription: Sample\n---\n"),
        )
        .unwrap();
        symlink(&skill, links.join(format!("{name}-skill"))).unwrap();
    }
    for name in ["claude", "codex", "pi"] {
        executable(&bin.join(name), "#!/bin/sh\necho '1.2.3'\n");
    }
    executable(
        &bin.join("git"),
        "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = status ]; then printf '%s\\n' \"$*\" >> \"$HOME/status.log\"; fi\n  if [ \"$arg\" = fetch ]; then\n    printf '%s\\n' \"$*\" >> \"$HOME/fetch.log\"\n    if [ \"$GIT_TERMINAL_PROMPT\" != 0 ]; then echo 'unexpected credential prompt' >&2; exit 2; fi\n    echo 'fatal: could not read Username: terminal prompts disabled' >&2\n    exit 1\n  fi\ndone\nexit 0\n",
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
    until(cx, "local checkout report", |cx| {
        cx.debug_bounds("agents-checkout-kit").is_some()
            && cx.debug_bounds("agents-checkout-extras").is_some()
    });
    assert!(
        !running.home.join("fetch.log").exists(),
        "opening the page must not fetch"
    );
    let click = |cx: &mut VisualTestContext, selector: &str| {
        let at = cx
            .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
            .expect(selector)
            .center();
        cx.simulate_click(at, Modifiers::none());
    };
    let failure = |name: &str| -> &'static str {
        Box::leak(format!("agents-sync-failure-{}-{name}", running.node).into_boxed_str())
    };
    let message = "agents-sync-message-Sync failed: fatal: could not read Username: terminal prompts disabled";
    assert!(cx.debug_bounds(failure("kit")).is_none());

    // A checkout offers its Sync; a failed one says so in its detail.
    click(cx, "agents-checkout-kit");
    let sync_kit = format!("agents-sync-{}-kit", running.node);
    until(cx, "sync kit", |cx| {
        cx.debug_bounds(Box::leak(sync_kit.clone().into_boxed_str()))
            .is_some()
    });
    click(cx, &sync_kit);
    until(cx, "kit's failure", |cx| {
        cx.debug_bounds(failure("kit")).is_some()
    });
    assert!(cx.debug_bounds(message).is_some(), "the failure in words");
    for name in ["claude", "codex", "pi"] {
        let available = Box::leak(format!("agents-tool-state-{name}-available").into_boxed_str());
        let broken = Box::leak(format!("agents-tool-state-{name}-broken").into_boxed_str());
        assert!(
            cx.debug_bounds(available).is_some(),
            "{name} should be available"
        );
        assert!(cx.debug_bounds(broken).is_none());
    }
    click(cx, "agents-checkout-extras");
    let sync_extras = format!("agents-sync-{}-extras", running.node);
    until(cx, "sync extras", |cx| {
        cx.debug_bounds(Box::leak(sync_extras.clone().into_boxed_str()))
            .is_some()
    });
    click(cx, &sync_extras);
    until(cx, "extras' failure", |cx| {
        cx.debug_bounds(failure("extras")).is_some()
    });

    // The overview has each failed checkout once, with its fix.
    click(cx, "agents-overview");
    until(cx, "both failures in the overview", |cx| {
        cx.debug_bounds(failure("kit")).is_some() && cx.debug_bounds(failure("extras")).is_some()
    });
    assert!(cx.debug_bounds("agents-ready").is_none());
    assert!(
        cx.debug_bounds(Box::leak(sync_kit.clone().into_boxed_str()))
            .is_some()
    );
    cx.run_until_parked();
    assert!(
        shell.read_with(cx, |shell, cx| shell
            .hub()
            .read(cx)
            .status
            .as_ref()
            .is_none_or(|status| !status.text.contains("could not read Username"))),
        "fetch details must not appear in an unsolicited status error"
    );
    assert_eq!(
        std::fs::read_to_string(running.home.join("fetch.log"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    let old_reads = std::fs::read_to_string(running.home.join("status.log"))
        .unwrap()
        .lines()
        .count();
    click(cx, "agents-refresh");
    until(cx, "local refresh completes", |_| {
        std::fs::read_to_string(running.home.join("status.log"))
            .is_ok_and(|text| text.lines().count() > old_reads)
    });
    until(cx, "failures persist through local refresh", |cx| {
        cx.debug_bounds(failure("kit")).is_some() && cx.debug_bounds(failure("extras")).is_some()
    });
    assert_eq!(
        std::fs::read_to_string(running.home.join("fetch.log"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    let _ = std::fs::remove_dir_all(settings);
}
