//! View control end to end: `parvi view show` and `parvi view open` against a
//! testkit node, with the headless shell connected to it showing two toy
//! pages defined here.

use gpui::{Context, TestAppContext, Window, div, prelude::*};
use parvi_app_kit::{Answers, Ask, Host, Mounted, Page, mount};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::time::{Duration, Instant};

thread_local! {
    static REVEALED: RefCell<Vec<Ask>> = const { RefCell::new(vec![]) };
}

/// Shown first.
struct Other;

impl Page for Other {
    const ID: &'static str = "other";
    const TITLE: &'static str = "Other";
}

impl Render for Other {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Answers reveals of paths, keeping what it was asked.
struct Files;

impl Page for Files {
    const ID: &'static str = "files";
    const TITLE: &'static str = "Files";
    const ANSWERS: &'static [Answers] = &[Answers::Reveal("path")];

    fn answer(&mut self, ask: Ask, _: &'static str, _: &mut Window, _: &mut Context<Self>) {
        REVEALED.with(|r| r.borrow_mut().push(ask));
    }
}

impl Render for Files {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn other(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
    mount(host, window, cx, |_, _, _| Other)
}

fn files(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
    mount(host, window, cx, |_, _, _| Files)
}

/// `parvi view <args> --json` against `url`, run on `rt` while the app
/// runs: its exit code and what it printed.
fn parvi(
    rt: &tokio::runtime::Runtime,
    cx: &mut gpui::VisualTestContext,
    url: &str,
    args: &[&str],
) -> (i32, Value) {
    let mut args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    args.extend(["--json".into(), "--node".into(), url.into()]);
    let running = rt.spawn(async move {
        let mut out = Vec::new();
        let code = parvi_node_kit::cli::run("view", &args, &mut out)
            .await
            .expect("the command runs");
        (code, serde_json::from_slice(&out).unwrap_or(Value::Null))
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !running.is_finished() {
        cx.run_until_parked();
        assert!(Instant::now() < deadline, "parvi view did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    rt.block_on(running).expect("the command's task")
}

#[gpui::test]
fn an_agent_opens_a_page_and_a_path_on_it_in_the_app(cx: &mut TestAppContext) {
    let rt = tokio::runtime::Runtime::new().expect("a runtime");
    let node = rt
        .block_on(parvi_testkit::node::Node::start(&[], ""))
        .expect("the node starts");
    let url = node.url();
    let settings = parvi_testkit::fresh_dir("parvi-app-view").join("app.json");
    // The node, the link and the command run on their own threads.
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let path = settings.clone();
    let dial = url.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[other, files], Some(dial), Some(path), window, cx)
    });

    // The app says what it shows once its link opens.
    let deadline = Instant::now() + Duration::from_secs(10);
    let shown = loop {
        let (code, shown) = parvi(&rt, cx, &url, &["show"]);
        assert_eq!(code, 0);
        if shown != json!([]) {
            break shown;
        }
        assert!(
            Instant::now() < deadline,
            "the app never said what it shows"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(shown[0]["page"], "other");

    // Opened by an agent: the page comes forward and is asked to reveal
    // the path.
    let (code, opened) = parvi(
        &rt,
        cx,
        &url,
        &["open", "--page", "files", "--path", "/tmp/x"],
    );
    assert_eq!(code, 0);
    assert_eq!(opened, json!({ "page": "files" }));
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("files"));
    let revealed = REVEALED.with(|r| r.borrow().clone());
    assert!(
        matches!(
            revealed.as_slice(),
            [Ask::Reveal { what, id, .. }] if what == "path" && id == "/tmp/x"
        ),
        "{revealed:?}"
    );

    rt.block_on(node.stop());
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}
