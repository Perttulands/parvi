//! Reporting from another page against an isolated Beads store.
#![cfg(unix)]
use gpui::{
    App, Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, div, prelude::*,
};
use parvi_app_kit::{Host, Mounted, Page, mount};
use parvi_node_kit::{Built, Listing, Setup};
use parvi_testkit::node::Node;
use std::sync::{OnceLock, mpsc};
static GH: OnceLock<String> = OnceLock::new();
static BD: OnceLock<String> = OnceLock::new();
fn build_work(setup: &Setup) -> anyhow::Result<Built> {
    parvi_work::build_with(
        setup,
        parvi_work::Bd::new(BD.get().unwrap()),
        parvi_work::Gh::new(GH.get().unwrap()),
    )
}
const WORK: Listing = Listing {
    build: build_work,
    ..parvi_work::LISTING
};
use std::time::{Duration, Instant};

struct Toy;
impl Page for Toy {
    const ID: &'static str = "toy";
    const TITLE: &'static str = "Toy";
}
impl Render for Toy {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("Toy")
    }
}
fn toy(host: Host, window: &mut Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, |_, _, _| Toy)
}

fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut ready: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if ready(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[gpui::test]
fn reports_file_to_beads_and_github_with_context_and_escape_discards(cx: &mut TestAppContext) {
    let dir = parvi_testkit::fresh_dir("parvi-report-store");
    std::fs::create_dir(dir.join(".beads")).unwrap();
    std::fs::write(dir.join(".beads/prefix"), "reporttest").unwrap();
    let bd = dir.join("fake-bd");
    std::fs::write(
        &bd,
        include_str!("../../node/tests/fixtures/work/fake-bd.sh"),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bd, std::fs::Permissions::from_mode(0o755)).unwrap();
    BD.set(bd.display().to_string()).unwrap();
    let gh = dir.join("fake-gh");
    let args = dir.join("gh-args");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../node/tests/fixtures/work/parvi.json");
    let item = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../node/tests/fixtures/work/item.json");
    std::fs::write(&gh, format!("#!/bin/sh\nif [ \"$1\" = issue ]; then printf '%s\\n' \"$@\" > '{}'; echo https://github.com/o/r/issues/51; else case \"$*\" in *number=51*) cat '{}';; *) cat '{}';; esac; fi\n", args.display(), item.display(), fixture.display())).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    GH.set(gh.display().to_string()).unwrap();
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let store = dir.clone();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let config = format!(
                "[work]\nprojects = [{:?}, {:?}]\n",
                format!("beads {}", store.display()),
                "github o/r"
            );
            let node = Node::start(&[WORK], &config).await.unwrap();
            let list = vec!["list".to_owned(), "--node".into(), node.url()];
            let mut output = Vec::new();
            assert_eq!(
                parvi_node_kit::cli::run("work", &list, &mut output)
                    .await
                    .unwrap(),
                0
            );
            assert!(String::from_utf8(output).unwrap().contains("gh:o/r#70"));
            let show = vec![
                "show".to_owned(),
                "--node".into(),
                node.url(),
                "--ref".into(),
                "gh:o/r#51".into(),
            ];
            let mut output = Vec::new();
            assert_eq!(
                parvi_node_kit::cli::run("work", &show, &mut output)
                    .await
                    .unwrap(),
                0
            );
            let output = String::from_utf8(output).unwrap();
            assert!(output.contains("gh:o/r#51") && output.contains("Example work item"));
            ready.send(node.url()).unwrap();
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            node.stop().await;
        });
    });
    let url = wait.recv().unwrap();
    let settings = parvi_testkit::fresh_dir("parvi-report-settings");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[toy, parvi_work_app::build],
            Some(url),
            Some(settings.join("app.json")),
            window,
            cx,
        )
    });
    until(cx, "link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == parvi_app_kit::link::client::LinkState::Open
        })
    });
    let host = shell.read_with(cx, |s, _| Host::new(s.hub()));
    let sessions = cx
        .update(|_, cx| host.model::<parvi_terminals_contract::SessionList>(cx))
        .unwrap();
    sessions.update(cx, |s, cx| {
        s.focused = Some(parvi_terminals_contract::FocusedSession {
            node: "node".into(),
            user: "user".into(),
            name: "alpha".into(),
        });
        cx.notify();
    });
    cx.simulate_keystrokes("alt-r");
    until(cx, "context", |cx| {
        cx.debug_bounds("report-context-Context: alpha").is_some()
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    until(cx, "project", |cx| {
        cx.debug_bounds("report-project-0").is_some()
    });
    cx.simulate_input("Title");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Body");
    cx.simulate_keystrokes("ctrl-enter");
    until(cx, "filed", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub()
                .read(cx)
                .status
                .as_ref()
                .is_some_and(|s| s.text.starts_with("Filed reporttest-"))
        })
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    let filed = dir.join(".beads/reporttest-1");
    assert_eq!(
        std::fs::read_to_string(filed.join("title")).unwrap(),
        "Title"
    );
    assert_eq!(
        std::fs::read_to_string(filed.join("description")).unwrap(),
        "Body\n\nContext: alpha"
    );
    assert_eq!(std::fs::read_to_string(filed.join("type")).unwrap(), "bug");
    cx.simulate_keystrokes("alt-r");
    until(cx, "GitHub project", |cx| {
        cx.debug_bounds("report-project-1").is_some()
    });
    let point = cx.debug_bounds("report-project-1").unwrap().center();
    cx.simulate_click(point, gpui::Modifiers::none());
    let point = cx.debug_bounds("report-field").unwrap().center();
    cx.simulate_click(point, gpui::Modifiers::none());
    cx.simulate_input("GitHub title");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("GitHub body");
    cx.simulate_keystrokes("ctrl-enter");
    until(cx, "GitHub filed", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub()
                .read(cx)
                .status
                .as_ref()
                .is_some_and(|s| s.text == "Filed 51")
        })
    });
    assert_eq!(
        std::fs::read_to_string(&args).unwrap(),
        "issue\ncreate\n--repo\no/r\n--title\nGitHub title\n--body\nGitHub body\n\nContext: alpha\n"
    );
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    cx.simulate_keystrokes("alt-r");
    until(cx, "report reopened", |cx| {
        cx.debug_bounds("report-box").is_some()
    });
    cx.simulate_input("Discard me");
    cx.simulate_keystrokes("escape");
    until(cx, "report closed", |cx| {
        cx.debug_bounds("report-box").is_none()
    });
    assert_eq!(
        std::fs::read_to_string(dir.join(".beads/count")).unwrap(),
        "1"
    );
    stop.send(()).unwrap();
    thread.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
