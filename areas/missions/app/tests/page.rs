#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::link::client::LinkState;
use parvi_node_kit::{Listing, NodeClient};
use parvi_testkit::{node::Node, sessions};
use serde_json::json;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const AREAS: &[Listing] = &[
    sessions::LISTING,
    parvi_work::LISTING,
    parvi_missions::LISTING,
];
struct Running {
    url: String,
    id: String,
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
fn start() -> Running {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let node = Node::start(AREAS, "").await.unwrap();
                let folder = node.dir().join("missions/missions");
                std::fs::create_dir_all(&folder).unwrap();
                std::fs::write(
                    folder.join("hello.mission.toml"),
                    include_str!("fixtures/solo-lab.mission.toml"),
                )
                .unwrap();
                ready.send((node.url(), node.id())).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                node.stop().await;
            });
    });
    let (url, id) = wait.recv().unwrap();
    Running {
        url,
        id,
        stop,
        thread: Some(thread),
    }
}
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut f: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        if f(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[gpui::test]
fn opens_a_fixture_mission_on_the_canvas(cx: &mut TestAppContext) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-missions-page-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (url, path) = (running.url.clone(), settings.clone());
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build],
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
    let selector = Box::leak(format!("mission-{}-hello", running.id).into_boxed_str());
    until(cx, "mission listing", |cx| {
        cx.debug_bounds(selector).is_some()
    });
    let at = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    until(cx, "step in flow", |cx| {
        cx.debug_bounds("flow-step-fmn_work").is_some()
    });
    let edit = cx.debug_bounds("flow-edit-fmn_work").unwrap().center();
    cx.simulate_click(edit, Modifiers::none());
    until(cx, "step editor", |cx| {
        cx.debug_bounds("step-goal").is_some()
    });
    let goal = cx.debug_bounds("step-goal").unwrap().center();
    cx.simulate_click(goal, Modifiers::none());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("Deliver the result");
    let save = cx.debug_bounds("step-save").unwrap().center();
    cx.simulate_click(save, Modifiers::none());
    until(cx, "save", |cx| cx.debug_bounds("step-goal").is_none());
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
        let (doc, _) = client
            .request(http::Method::GET, "/api/missions/hello", None, None)
            .await
            .unwrap();
        assert_eq!(
            doc["mission"]["formations"][0]["brief"]["goal"],
            "Deliver the result"
        );
    });
    let mode = cx.debug_bounds("mission-mode-canvas").unwrap().center();
    cx.simulate_click(mode, Modifiers::none());
    until(cx, "step on canvas", |cx| {
        cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    assert!(cx.debug_bounds("mission-card-inp_hello").is_some());
    let card = cx.debug_bounds("mission-card-fmn_work").unwrap().center();
    cx.simulate_click(card, Modifiers::none());
    until(cx, "editor reopened", |cx| {
        cx.debug_bounds("step-goal").is_some()
    });
    let goal = cx.debug_bounds("step-goal").unwrap().center();
    cx.simulate_click(goal, Modifiers::none());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("Keep this draft");
    rt.block_on(async {
        let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
        let (doc, etag) = client
            .request(http::Method::GET, "/api/missions/hello", None, None)
            .await
            .unwrap();
        client
            .request(
                http::Method::PATCH,
                "/api/missions/hello",
                Some(json!({"expectedRev": doc["mission"]["rev"], "title":"Changed elsewhere"})),
                Some(&etag),
            )
            .await
            .unwrap();
    });
    let save = cx.debug_bounds("step-save").unwrap().center();
    cx.simulate_click(save, Modifiers::none());
    until(cx, "merged save", |cx| {
        cx.debug_bounds("step-goal").is_none()
    });
    rt.block_on(async {
        let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
        let (doc, _) = client
            .request(http::Method::GET, "/api/missions/hello", None, None)
            .await
            .unwrap();
        assert_eq!(doc["mission"]["title"], "Changed elsewhere");
        let formation = doc["mission"]["formations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "fmn_work")
            .unwrap();
        assert_eq!(formation["brief"]["goal"], "Keep this draft");
    });
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}

// The node is authored exclusively through the public CLI, then opened by the app.
#[gpui::test]
fn cli_authored_graph_and_layout_open_on_the_canvas(cx: &mut TestAppContext) {
    let running = start();
    let rt = tokio::runtime::Runtime::new().unwrap();
    async fn cli(url: &str, noun: &str, args: &[&str]) -> serde_json::Value {
        let mut argv: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        argv.extend(["--node".into(), format!("{url}/"), "--json".into()]);
        let mut output = Vec::new();
        let exit = parvi_node_kit::cli::run(noun, &argv, &mut output)
            .await
            .unwrap();
        assert_eq!(
            exit,
            0,
            "{noun} {argv:?}: {}",
            String::from_utf8_lossy(&output)
        );
        serde_json::from_slice(&output).unwrap()
    }
    let (input, steps, judge, human, end) = rt.block_on(async {
        let u = &running.url;
        cli(u, "mission", &["new", "cli-board", "--title", "CLI Board"]).await;
        let input = cli(u, "mission", &["create", "cli-board", "--title", "Start"]).await["inputCard"]["id"].as_str().unwrap().to_owned();
        let mut steps = Vec::new();
        for title in ["Draft", "Polish", "Judge"] {
            let step = cli(u, "formation", &["create", "cli-board", "--type", "solo", "--title", title]).await["formation"].clone();
            let id = step["id"].as_str().unwrap().to_owned();
            let input = format!("{id}:{}", step["inputs"][0]["id"].as_str().unwrap());
            let output = format!("{id}:{}", step["outputs"][0]["id"].as_str().unwrap());
            steps.push((id, input, output));
        }
        let judge = cli(u, "gate", &["create", "cli-board", "--title", "Judge gate", "--kinds", "formation"]).await["gate"]["id"].as_str().unwrap().to_owned();
        cli(u, "gate", &["judge", "cli-board", &judge, "--chain", &steps[2].0]).await;
        let human = cli(u, "gate", &["create", "cli-board", "--title", "Human gate"]).await["gate"]["id"].as_str().unwrap().to_owned();
        let end = cli(u, "end", &["create", "cli-board", "--outcome", "done"]).await["end"]["id"].as_str().unwrap().to_owned();
        cli(u, "mission", &["wire", "cli-board", &steps[0].1, "--input", &input]).await;
        for (from, to) in [
            (steps[0].2.clone(), format!("{judge}:in")),
            (format!("{judge}:pass"), steps[1].1.clone()),
            (steps[1].2.clone(), format!("{human}:in")),
            (format!("{human}:pass"), format!("{end}:in")),
        ] {
            cli(u, "formation", &["wire", "cli-board", &from, &to]).await;
        }
        cli(u, "mission", &["move", "cli-board", &steps[0].0, "--x", "123", "--y", "456"]).await;
        (input, steps, judge, human, end)
    });
    let settings = std::env::temp_dir()
        .join(format!("parvi-cli-canvas-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    let selector = Box::leak(format!("mission-{}-cli-board", running.id).into_boxed_str());
    until(cx, "CLI mission listing", |cx| {
        cx.debug_bounds(selector).is_some()
    });
    let at = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    until(cx, "canvas control", |cx| {
        cx.debug_bounds("mission-mode-canvas").is_some()
    });
    let at = cx.debug_bounds("mission-mode-canvas").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    for id in std::iter::once(&input)
        .chain(steps.iter().map(|s| &s.0))
        .chain([&judge, &human, &end])
    {
        let selector = Box::leak(format!("mission-card-{id}").into_boxed_str());
        until(cx, "CLI-authored canvas card", |cx| {
            cx.debug_bounds(selector).is_some()
        });
    }
    let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
    let layout = rt
        .block_on(client.request(
            http::Method::GET,
            "/api/missions/cli-board/layout",
            None,
            None,
        ))
        .unwrap()
        .0;
    assert!(
        layout["layout"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == steps[0].0 && node["x"] == 123 && node["y"] == 456)
    );
}
