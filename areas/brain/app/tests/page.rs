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
    let (saved, saved_text) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let corpus = fresh_dir("brain-page-corpus");
            std::fs::create_dir(corpus.join("notes")).unwrap();
            std::fs::write(
                corpus.join("notes/page.md"),
                "---\ntags: [shared]\n---\n# Test page\n\n[[zlink]]\n",
            )
            .unwrap();
            std::fs::write(corpus.join("notes/zlink.md"), "# Link\n").unwrap();
            std::fs::write(corpus.join("notes/zback.md"), "# Backlink\n\n[[page]]\n").unwrap();
            std::fs::write(
                corpus.join("notes/ztag.md"),
                "---\ntags:\n  - Shared\n---\n# Tagged\n",
            )
            .unwrap();
            for i in 0..4 {
                std::fs::write(corpus.join(format!("notes/zfiller{i}.md")), "# Filler\n").unwrap();
            }
            let git = |args: &[&str]| {
                let output = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&corpus)
                    .args(args)
                    .env("GIT_AUTHOR_DATE", "2020-02-03T04:05:06Z")
                    .env("GIT_COMMITTER_DATE", "2020-02-03T04:05:06Z")
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            };
            git(&["init", "-q"]);
            git(&["config", "user.name", "Corpus Owner"]);
            git(&["config", "user.email", "author@example.org"]);
            git(&["add", "."]);
            git(&["commit", "-qm", "Initial page"]);
            let config = format!(
                "[brain]\ncorpus = {:?}\nauthor = \"perttu <author@example.org>\"\n",
                corpus.to_str().unwrap()
            );
            let node = Node::start(&[parvi_brain::LISTING], &config).await.unwrap();
            ready.send(node.url()).unwrap();
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            node.stop().await;
            let contents = std::fs::read_to_string(corpus.join("notes/page.md")).unwrap();
            let log = std::process::Command::new("git")
                .arg("-C")
                .arg(&corpus)
                .args(["log", "-1", "--format=%an|%s"])
                .output()
                .unwrap();
            assert!(log.status.success());
            saved
                .send((contents, String::from_utf8(log.stdout).unwrap()))
                .unwrap();
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
    let when = parvi_brain_app::state::when("2020-02-03T04:05:06Z", jiff::Timestamp::now());
    let meta =
        Box::leak(format!("brain-meta-notes · changed {when} by Corpus Owner").into_boxed_str());
    let head = cx
        .debug_bounds(meta)
        .expect("subject, time and author on the metadata line");
    let history = cx.debug_bounds("brain-history").expect("History button");
    assert_eq!(
        head.origin.y,
        history.origin.y + (history.size.height - head.size.height) / 2.
    );
    assert!(cx.debug_bounds("brain-history-open").is_none());
    assert!(cx.debug_bounds("brain-links").is_some());
    assert!(cx.debug_bounds("brain-link-notes/zlink.md").is_some());
    assert!(cx.debug_bounds("brain-backlinks").is_some());
    assert!(cx.debug_bounds("brain-backlink-notes/zback.md").is_some());
    assert!(cx.debug_bounds("brain-related").is_some());
    assert!(cx.debug_bounds("brain-related-notes/ztag.md").is_some());
    let history = history.center();
    cx.simulate_click(history, Modifiers::none());
    until(cx, "history unfolded", |cx| {
        cx.debug_bounds("brain-history-open").is_some()
    });
    cx.simulate_click(history, Modifiers::none());
    until(cx, "history folded", |cx| {
        cx.debug_bounds("brain-history-open").is_none()
    });
    let edit = cx.debug_bounds("brain-edit").unwrap().center();
    cx.simulate_click(edit, Modifiers::none());
    until(cx, "editor", |cx| cx.debug_bounds("brain-save").is_some());
    cx.simulate_keystrokes("ctrl-end");
    cx.simulate_input("Saved from Brain");
    let save = cx.debug_bounds("brain-save").unwrap().center();
    cx.simulate_click(save, Modifiers::none());
    until(cx, "save to finish", |cx| {
        cx.debug_bounds("brain-edit").is_some()
    });
    stop.send(()).unwrap();
    thread.join().unwrap();
    let (contents, log) = saved_text.recv().unwrap();
    assert!(contents.contains("Saved from Brain"));
    assert_eq!(log.trim(), "perttu|Edit Test page");
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
