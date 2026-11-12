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
            std::fs::create_dir(corpus.join("other")).unwrap();
            std::fs::write(corpus.join("other/third.md"), "# Third\n").unwrap();
            std::fs::write(corpus.join("notes/first.md"), "# First\n\n[[second|Second]]\n").unwrap();
            std::fs::write(corpus.join("notes/second.md"), "# Second\n\nLinked page.\n").unwrap();
            std::fs::write(corpus.join("notes/long.md"), format!("# {}\n", "A very long arrival title ".repeat(12))).unwrap();
            let git = |args: &[&str]| {
                let output = std::process::Command::new("git").arg("-C").arg(&corpus).args(args).output().unwrap();
                assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            };
            git(&["init", "-q"]);
            git(&["config", "user.name", "Fixture"]);
            git(&["config", "user.email", "fixture@example.org"]);
            git(&["add", "."]);
            git(&["commit", "-qm", "Fixture"]);
            let citadel = axum::Router::new().route("/v1/library/search", post(|Json(query): Json<Value>| async move {
                let results = if query["query"] == "first" { json!([
                    { "path": "notes/first.md", "title": "First", "collection": "notes", "kind": "note", "summary": "A fixture summary" },
                    { "path": "notes/second.md", "title": "Second", "collection": "notes" },
                    { "path": "other/third.md", "title": "Third", "collection": "other" }
                ]) } else { json!([]) };
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
    until(cx, "long arrival", |cx| {
        cx.debug_bounds("brain-rail-page-notes/long.md").is_some()
    });
    let title = cx.debug_bounds("brain-rail-page-notes/long.md").unwrap();
    let date = cx.debug_bounds("brain-rail-date-notes/long.md").unwrap();
    assert!(
        title.right() <= date.left(),
        "arrival title leaves room for its date"
    );
    let search = cx.debug_bounds("brain-search").unwrap().center();
    cx.simulate_click(search, Modifiers::none());
    cx.simulate_input("first");
    cx.executor().advance_clock(Duration::from_millis(250));
    until(cx, "search result", |cx| {
        cx.debug_bounds("brain-result-0").is_some()
    });
    assert!(cx.debug_bounds("brain-result-selected-0").is_some());
    let second = cx.debug_bounds("brain-result-1").unwrap().center();
    cx.simulate_click(second, Modifiers::none());
    until(cx, "selected second result", |cx| {
        cx.debug_bounds("brain-result-selected-1").is_some()
    });
    assert!(cx.debug_bounds("brain-result-selected-0").is_none());
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
    until(cx, "the trail back to the first page", |cx| {
        cx.debug_bounds("brain-trail-notes/first.md").is_some()
    });
    let back = cx
        .debug_bounds("brain-trail-notes/first.md")
        .unwrap()
        .center();
    cx.simulate_click(back, Modifiers::none());
    until(cx, "back along trail", |cx| {
        cx.debug_bounds("brain-reading-notes/first.md").is_some()
    });
    assert!(cx.debug_bounds("brain-trail-notes/second.md").is_none());
    std::fs::remove_dir_all(settings).unwrap();
}

thread_local! {
    static SENT: std::cell::RefCell<Option<parvi_app_kit::Ask>> = const { std::cell::RefCell::new(None) };
}

struct Toy;
impl parvi_app_kit::Page for Toy {
    const ID: &'static str = "toy";
    const TITLE: &'static str = "Toy";
    const ANSWERS: &'static [parvi_app_kit::Answers] = &[parvi_app_kit::Answers::Send];
    fn answer(
        &mut self,
        ask: parvi_app_kit::Ask,
        _: &'static str,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) {
        SENT.with(|sent| *sent.borrow_mut() = Some(ask));
    }
}
impl gpui::Render for Toy {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::prelude::*;
        gpui::div()
            .size_full()
            .debug_selector(|| "toy-page".into())
            .child("Toy")
    }
}
fn toy(
    host: parvi_app_kit::Host,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> parvi_app_kit::Mounted {
    parvi_app_kit::mount(host, window, cx, |_, _, _| Toy)
}

#[gpui::test]
fn dock_search_send_and_toggle_from_another_page(cx: &mut TestAppContext) {
    let running = start();
    let settings = fresh_dir("brain-dock-settings");
    SENT.with(|sent| *sent.borrow_mut() = None);
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (shell, cx) = cx.add_window_view(|window, cx| {
        parvi_app_shell::start(
            &[toy, parvi_brain_app::build],
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
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    let chords = shell.update_in(cx, |shell, _, cx| shell.chords(cx));
    assert!(
        parvi_app_kit::chords::rows(&chords)
            .iter()
            .any(|row| row.label == "Brain")
    );
    cx.simulate_keystrokes("alt-l");
    until(cx, "brain dock", |cx| {
        cx.debug_bounds("dock-brain").is_some()
    });
    assert!(cx.debug_bounds("toy-page").is_some());
    let search = cx.debug_bounds("brain-search").unwrap().center();
    cx.simulate_click(search, Modifiers::none());
    cx.simulate_input("first");
    cx.executor().advance_clock(Duration::from_millis(250));
    until(cx, "search result", |cx| {
        cx.debug_bounds("brain-result-0").is_some()
    });
    assert!(cx.debug_bounds("brain-result-kind-note").is_some());
    assert!(
        cx.debug_bounds("brain-result-summary-A fixture summary")
            .is_some()
    );
    cx.simulate_keystrokes("down");
    until(cx, "down opens second hit", |cx| {
        cx.debug_bounds("brain-reading-notes/second.md").is_some()
    });
    cx.simulate_keystrokes("up");
    until(cx, "up opens first hit", |cx| {
        cx.debug_bounds("brain-reading-notes/first.md").is_some()
    });
    cx.simulate_keystrokes("escape");
    until(cx, "search cleared", |cx| {
        cx.debug_bounds("brain-result-0").is_none()
    });
    assert!(cx.debug_bounds("dock-brain").is_some());
    cx.simulate_keystrokes("escape");
    until(cx, "dock closed by Escape", |cx| {
        cx.debug_bounds("dock-brain").is_none()
    });
    assert!(cx.debug_bounds("toy-page").is_some());
    cx.simulate_keystrokes("alt-l");
    until(cx, "dock reopened", |cx| {
        cx.debug_bounds("brain-search").is_some()
    });
    let search = cx.debug_bounds("brain-search").unwrap().center();
    cx.simulate_click(search, Modifiers::none());
    cx.simulate_input("first");
    cx.executor().advance_clock(Duration::from_millis(250));
    until(cx, "search result again", |cx| {
        cx.debug_bounds("brain-result-0").is_some()
    });
    let result = cx.debug_bounds("brain-result-0").unwrap().center();
    cx.simulate_click(result, Modifiers::none());
    until(cx, "page in dock", |cx| {
        cx.debug_bounds("brain-reading-notes/first.md").is_some()
    });
    let send = cx.debug_bounds("brain-send").unwrap().center();
    cx.simulate_click(send, Modifiers::none());
    assert_eq!(
        SENT.with(|sent| sent.borrow().clone()),
        Some(parvi_app_kit::Ask::Send {
            reference: "library notes/first.md".into(),
            target: None
        })
    );
    cx.simulate_keystrokes("alt-l");
    until(cx, "dock closed", |cx| {
        cx.debug_bounds("dock-brain").is_none()
    });
    assert!(cx.debug_bounds("toy-page").is_some());
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    std::fs::remove_dir_all(settings).unwrap();
}

#[gpui::test]
fn hiding_a_collection_removes_its_arrivals_and_search_hits(cx: &mut TestAppContext) {
    let running = start();
    let settings = fresh_dir("brain-hide-journey-settings");
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
    until(cx, "notes arrival", |cx| {
        cx.debug_bounds("brain-rail-page-notes/first.md").is_some()
    });
    assert!(cx.debug_bounds("brain-rail-page-other/third.md").is_some());
    let hide = cx.debug_bounds("brain-hide-0").unwrap().center();
    cx.simulate_click(hide, Modifiers::none());
    until(cx, "hidden arrivals removed", |cx| {
        cx.debug_bounds("brain-rail-page-notes/first.md").is_none()
    });
    assert!(cx.debug_bounds("brain-rail-page-other/third.md").is_some());
    let search = cx.debug_bounds("brain-search").unwrap().center();
    cx.simulate_click(search, Modifiers::none());
    cx.simulate_input("first");
    cx.executor().advance_clock(Duration::from_millis(250));
    until(cx, "other search hit", |cx| {
        cx.debug_bounds("brain-hit-other/third.md").is_some()
    });
    assert!(cx.debug_bounds("brain-hit-notes/first.md").is_none());
    assert!(cx.debug_bounds("brain-hit-notes/second.md").is_none());
    std::fs::remove_dir_all(settings).unwrap();
}
