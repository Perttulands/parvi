#![cfg(unix)]
use gpui::{
    App, AppContext, Context, IntoElement, Modifiers, ParentElement, TestAppContext,
    VisualTestContext, Window, div, point, px,
};
use parvi_app_kit::{Ask, Host, Mounted, Page, ask::Answers, mount};
use parvi_core::NodeId;
use parvi_mission_engine::{
    ExecutionSink, Executor, FormationExecution, FormationResult, LabExecutor,
};
use parvi_node_kit::{Built, Listing, Setup};
use parvi_testkit::{node::Node, sessions};
use serde_json::{Value, json};
use std::{
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

static OPENED: Mutex<Vec<Ask>> = Mutex::new(Vec::new());
/// Answers as Files does: a beside ask opens a view beside the page that
/// asked, which that page must draw.
struct ToyFiles(Host);
struct Opened;
impl gpui::Render for Opened {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child("Opened file")
    }
}
impl gpui::Render for ToyFiles {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child("Files")
    }
}
impl Page for ToyFiles {
    const ID: &'static str = "toy-files";
    const TITLE: &'static str = "Files";
    const ANSWERS: &'static [Answers] = &[Answers::OpenPath];
    fn answer(&mut self, ask: Ask, from: &'static str, _: &mut Window, cx: &mut Context<Self>) {
        if ask.beside() {
            let view = cx.new(|_| Opened);
            self.0.show_beside(from, view.into(), cx);
        }
        OPENED.lock().unwrap().push(ask);
    }
}
fn toy_files(host: Host, window: &mut Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, |host, _, _| ToyFiles(host))
}
struct Scripted;
impl Executor for Scripted {
    fn execute_formation(
        &self,
        request: FormationExecution,
        sink: ExecutionSink,
    ) -> Result<FormationResult, parvi_core::Error> {
        let judge = request.formation.id == "fmn_judge";
        let attempt = request.attempt;
        let path = request.artifact_dir.join("result.png");
        let mut result = LabExecutor.execute_formation(request, sink)?;
        if judge {
            let verdict = if attempt == 1 { "fail" } else { "pass" };
            let reason = if attempt == 1 {
                "add coverage"
            } else {
                "covered"
            };
            let text = format!(
                "```parvi-verdict\n{}\n```",
                json!({"verdict":verdict,"reason":reason,"evidence":[]})
            );
            result.text = text.clone();
            for output in result.outputs.values_mut() {
                output["text"] = json!(text);
            }
        } else {
            std::fs::write(&path, include_bytes!("fixtures/output.png")).unwrap();
            result.outputs["port_out"]["ref"] = json!(path);
        }
        Ok(result)
    }
}
fn build(setup: &Setup) -> anyhow::Result<Built> {
    parvi_missions::build_with_executor(setup, Some(Box::new(Scripted)))
}
const MISSIONS: Listing = Listing {
    build,
    ..parvi_missions::LISTING
};
fn mission() -> String {
    let solo = include_str!("fixtures/solo-lab.mission.toml");
    let judge = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_judge")
        .replace("slot_work", "slot_judge");
    let work = solo
        .replace("label = \"Result\"", "label = \"Result\"\nkind = \"file\"")
        .replace("to = \"end_done:in\"", "to = \"gate_review:in\"");
    format!(
        "{work}\n[[formation]]{judge}\n[[gate]]\nid = \"gate_review\"\ntitle = \"Review\"\nkinds = [\"formation\"]\n[[connection]]\nid = \"edge_pass\"\nfrom = \"gate_review:pass\"\nto = \"end_done:in\"\n[[connection]]\nid = \"edge_fail\"\nfrom = \"gate_review:fail\"\nto = \"fmn_work:port_in\"\n[[connection]]\nid = \"edge_judge\"\nfrom = \"gate_review:judge\"\nto = \"fmn_judge:port_in\"\n[[connection]]\nid = \"edge_return\"\nfrom = \"fmn_judge:port_out\"\nto = \"gate_review:judge\"\n"
    )
}
struct Running {
    url: String,
    id: String,
    run: String,
    evidence: Value,
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
    let (tx, rx) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
        let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, parvi_files::LISTING, MISSIONS], "[files]\nroots = ['/tmp']\n").await.unwrap();
        let dir = node.dir().join("missions/missions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hello.mission.toml"), mission()).unwrap();
        let result = node.call("mission_run", json!({"mission":"hello","input":"inp_hello","inputs":{"brief":"ship"},"cwd":node.dir()})).await.unwrap();
        let id = result["runId"].as_str().unwrap().to_owned();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let state = node.call("run_status", json!({"run":id})).await.unwrap();
            if state["final"] == true || state["status"] == "blocked" { assert_eq!(state["status"], "succeeded", "{state}"); break; }
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let evidence = node.call("run_evidence", json!({"run":id})).await.unwrap();
        tx.send((node.url(), node.id(), id, evidence)).unwrap();
        while stopped.try_recv().is_err() { tokio::time::sleep(Duration::from_millis(20)).await; }
        node.stop().await;
    })
    });
    let (url, id, run, evidence) = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    Running {
        url,
        id,
        run,
        evidence,
        stop,
        thread: Some(thread),
    }
}
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut f: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if f(cx) {
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn click(cx: &mut VisualTestContext, selector: &str) {
    let bounds = cx
        .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .expect(selector);
    cx.simulate_click(bounds.center(), Modifiers::none());
}
fn reveal_control(cx: &mut VisualTestContext, selector: &str) {
    let selector: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    for _ in 0..10 {
        let Some(bounds) = cx.debug_bounds(selector) else {
            cx.run_until_parked();
            continue;
        };
        let y = f32::from(bounds.center().y);
        if (100. ..800.).contains(&y) {
            return;
        }
        cx.simulate_event(gpui::ScrollEvent {
            position: point(px(700.), px(500.)),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px((500. - y).clamp(-400., 400.)))),
            modifiers: Default::default(),
            touch_phase: gpui::TouchPhase::Moved,
            source: gpui::ScrollSource::Pointer,
        });
        cx.run_until_parked();
    }
}
#[gpui::test]
fn evidence_links_and_attempt_images(cx: &mut TestAppContext) {
    OPENED.lock().unwrap().clear();
    let running = start();
    cx.executor().allow_parking();
    cx.update(|cx| {
        parvi_app_shell::prepare(cx);
        cx.set_app_identity("io.parvi.evidence-test", "Parvi evidence test");
    });
    let settings = std::env::temp_dir()
        .join(format!("parvi-evidence-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build, toy_files],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    shell.update(cx, |shell, cx| {
        Host::new(shell.hub())
            .for_page("test", &[])
            .ask(
                Ask::Reveal {
                    what: "mission".into(),
                    id: "hello".into(),
                    node: Some(NodeId(running.id.clone())),
                    part: None,
                    beside: false,
                },
                cx,
            )
            .unwrap();
    });
    until(cx, "mission flow", |cx| {
        cx.debug_bounds("flow-step-fmn_work").is_some()
    });
    click(cx, "mission-mode-canvas");
    until(cx, "mission", |cx| {
        cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    shell.update(cx, |shell, cx| {
        Host::new(shell.hub())
            .for_page("test", &[])
            .ask(
                Ask::Reveal {
                    what: "run".into(),
                    id: running.run.clone(),
                    node: Some(NodeId(running.id.clone())),
                    part: None,
                    beside: false,
                },
                cx,
            )
            .unwrap();
    });
    let e = &running.evidence["evidence"];
    let problem = e["problems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["reason"]["text"] == "add coverage")
        .unwrap();
    let problem_selector = format!(
        "run-problem-{}-add coverage",
        problem["seq"].as_u64().unwrap()
    );
    until(cx, "failed verdict", |cx| {
        cx.debug_bounds(Box::leak(problem_selector.clone().into_boxed_str()))
            .is_some()
    });
    let files = e["files"].as_array().unwrap();
    let (index, file) = files
        .iter()
        .enumerate()
        .find(|(_, f)| f["path"].as_str().unwrap().ends_with("result.png"))
        .unwrap();
    let file_path = file["path"].as_str().unwrap().to_owned();
    let file_selector = format!("run-file-{index}");
    until(cx, "file", |cx| {
        cx.debug_bounds(Box::leak(file_selector.clone().into_boxed_str()))
            .is_some()
    });
    click(cx, &file_selector);
    until(cx, "file ask", |_| !OPENED.lock().unwrap().is_empty());
    until(cx, "the file shown beside the run", |cx| {
        cx.debug_bounds("missions-beside").is_some()
    });
    assert_eq!(
        OPENED.lock().unwrap().last().unwrap(),
        &Ask::OpenPath {
            node: NodeId(running.id.clone()),
            path: file_path,
            beside: true
        }
    );
    let brief = &e["briefs"][0];
    let brief_selector = format!("run-brief-{}", brief["dispatchSeq"].as_u64().unwrap());
    click(cx, &brief_selector);
    until(cx, "brief ask", |_| OPENED.lock().unwrap().len() >= 2);
    assert_eq!(
        OPENED.lock().unwrap().last().unwrap(),
        &Ask::OpenPath {
            node: NodeId(running.id.clone()),
            path: brief["path"].as_str().unwrap().into(),
            beside: true
        }
    );
    reveal_control(cx, "run-mode-canvas");
    click(cx, "run-mode-canvas");
    until(cx, "run canvas attempts", |cx| {
        cx.debug_bounds("card-attempts-fmn_work").is_some()
    });
    reveal_control(cx, "card-attempts-fmn_work");
    click(cx, "card-attempts-fmn_work");
    until(cx, "canvas attempts window", |cx| {
        cx.debug_bounds("attempts-window").is_some()
    });
    reveal_control(cx, "attempts-close");
    click(cx, "attempts-close");
    until(cx, "canvas attempts closed", |cx| {
        cx.debug_bounds("attempts-window").is_none()
    });
    reveal_control(cx, "run-mode-flow");
    click(cx, "run-mode-flow");
    until(cx, "flow view", |cx| {
        cx.debug_bounds("run-step-fmn_work-done").is_some()
    });
    reveal_control(cx, "run-step-attempts-fmn_work");
    click(cx, "run-step-attempts-fmn_work");
    until(cx, "flow attempts window", |cx| {
        cx.debug_bounds("attempts-window").is_some()
    });
    click(cx, "attempts-close");
    until(cx, "flow attempts closed", |cx| {
        cx.debug_bounds("attempts-window").is_none()
    });
    cx.simulate_event(gpui::ScrollEvent {
        position: point(px(700.), px(500.)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-800.))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
        source: gpui::ScrollSource::Pointer,
    });
    cx.run_until_parked();
    reveal_control(cx, "run-step-attempts-fmn_work");
    click(cx, "run-step-attempts-fmn_work");
    until(cx, "compare button", |cx| {
        cx.debug_bounds("attempt-compare").is_some()
    });
    click(cx, "attempt-compare");
    until(cx, "both decoded images", |cx| {
        cx.debug_bounds("attempt-image-1").is_some() && cx.debug_bounds("attempt-image-2").is_some()
    });
    let a = cx.debug_bounds("attempt-image-1").unwrap();
    let b = cx.debug_bounds("attempt-image-2").unwrap();
    assert!(
        a.origin.x < b.origin.x,
        "images must be side by side: {a:?}, {b:?}"
    );
    cx.simulate_keystrokes("c");
    until(cx, "single attempt", |cx| {
        cx.debug_bounds("attempt-card-1").is_none() && cx.debug_bounds("attempt-card-2").is_some()
    });
    cx.simulate_keystrokes("left");
    until(cx, "previous attempt", |cx| {
        cx.debug_bounds("attempt-card-1").is_some() && cx.debug_bounds("attempt-card-2").is_none()
    });
    cx.simulate_keystrokes("end");
    until(cx, "last attempt", |cx| {
        cx.debug_bounds("attempt-card-2").is_some()
    });
    cx.simulate_keystrokes("home");
    until(cx, "first attempt", |cx| {
        cx.debug_bounds("attempt-card-1").is_some()
    });
    cx.simulate_keystrokes("2");
    until(cx, "numbered attempt", |cx| {
        cx.debug_bounds("attempt-card-2").is_some()
    });
    cx.simulate_keystrokes("shift-left");
    until(cx, "keyboard comparison", |cx| {
        cx.debug_bounds("attempt-card-1").is_some() && cx.debug_bounds("attempt-card-2").is_some()
    });
    cx.simulate_keystrokes("escape");
    until(cx, "closed attempts", |cx| {
        cx.debug_bounds("attempts-window").is_none()
    });
}
