#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::{Ask, Host, link::client::LinkState};
use parvi_core::NodeId;
use parvi_mission_engine::{
    ExecutionSink, Executor, FormationExecution, FormationResult, LabExecutor,
};
use parvi_node_kit::{Built, Listing, NodeClient, Setup};
use parvi_term::Sessions;
use parvi_testkit::{
    node::Node,
    sessions::{self, FakeSessions},
};
use serde_json::json;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

static FAKE: Mutex<Option<Arc<FakeSessions>>> = Mutex::new(None);
static RELEASE: Mutex<Option<mpsc::Receiver<()>>> = Mutex::new(None);
struct FakeSeatLab {
    runtime: tokio::runtime::Handle,
}
impl Executor for FakeSeatLab {
    fn execute_formation(
        &self,
        execution: FormationExecution,
        sink: ExecutionSink,
    ) -> Result<FormationResult, parvi_core::Error> {
        let sessions = FAKE.lock().unwrap().clone().expect("fake Sessions");
        let id = sessions.add(&format!("seat-{}", execution.formation.id));
        self.runtime
            .block_on(async {
                let pane = sessions.active_pane(&id).await?;
                sessions
                    .deliver(pane, execution.input_text.clone(), true)
                    .await?;
                Ok::<_, anyhow::Error>(())
            })
            .map_err(|e| parvi_core::Error::new("test", e.to_string()))?;
        let receiver = RELEASE.lock().unwrap().take().expect("release receiver");
        receiver
            .recv_timeout(Duration::from_secs(20))
            .expect("test releases step");
        LabExecutor.execute_formation(execution, sink)
    }
}
fn build(setup: &Setup) -> anyhow::Result<Built> {
    parvi_missions::build_with_executor(
        setup,
        Some(Box::new(FakeSeatLab {
            runtime: tokio::runtime::Handle::current(),
        })),
    )
}
const MISSIONS: Listing = Listing {
    build,
    ..parvi_missions::LISTING
};
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
            let _ = t.join();
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
                let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, MISSIONS], "")
                    .await
                    .unwrap();
                *FAKE.lock().unwrap() = Some(node.fake_sessions());
                let dir = node.dir().join("missions/missions");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(
                    dir.join("gated.mission.toml"),
                    include_str!("fixtures/gated.mission.toml"),
                )
                .unwrap();
                std::fs::write(
                    dir.join("admission-findings.mission.toml"),
                    include_str!("fixtures/admission-findings.mission.toml"),
                )
                .unwrap();
                ready.send((node.url(), node.id())).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                *FAKE.lock().unwrap() = None;
                node.stop().await;
            })
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
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if f(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = cx
        .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .expect(selector)
        .center();
    cx.simulate_click(at, Modifiers::none());
}
#[gpui::test]
fn starts_a_run_sees_active_step_answers_gate_and_shows_admission_findings(
    cx: &mut TestAppContext,
) {
    let (release, receiver) = mpsc::channel();
    *RELEASE.lock().unwrap() = Some(receiver);
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-missions-run-{}", std::process::id()))
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
    // The Agents library asks to open a mission before its list has arrived.
    shell.update(cx, |s, cx| {
        let host = Host::new(s.hub()).for_page("agents", &[]);
        host.ask(
            Ask::Reveal {
                what: "mission".into(),
                id: "gated".into(),
                node: Some(NodeId(running.id.clone())),
                part: None,
            },
            cx,
        )
        .unwrap();
    });
    until(cx, "link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    until(cx, "mission opened by role ask", |cx| {
        cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    click(cx, "mission-card-fmn_work");
    until(cx, "step editor", |cx| {
        cx.debug_bounds("step-goal").is_some()
    });
    assert!(
        shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    );
    click(cx, "step-goal");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("Temporary");
    cx.run_until_parked();
    assert!(
        !shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    );
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("Do the work");
    cx.run_until_parked();
    assert!(
        shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    );
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("Deliver reviewed work");
    cx.run_until_parked();
    assert!(
        !shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    );
    assert!(!cx.simulate_close());
    cx.run_until_parked();
    assert!(cx.debug_bounds("close-guard").is_some());
    click(cx, "close-stay");
    // A reveal cannot discard the draft; it is fulfilled after saving.
    shell.update(cx, |s, cx| {
        Host::new(s.hub())
            .for_page("test", &[])
            .ask(
                Ask::Reveal {
                    what: "mission".into(),
                    id: "gated".into(),
                    node: None,
                    part: Some("fmn_work".into()),
                },
                cx,
            )
            .unwrap();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("step-goal").is_some());
    click(cx, "step-save");
    until(cx, "saved step", |cx| {
        cx.debug_bounds("step-goal").is_none()
    });
    until(cx, "queued mission reveal", |cx| {
        cx.debug_bounds("mission-reveal-fmn_work").is_some()
    });
    assert!(
        shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    );
    let client_runtime = tokio::runtime::Runtime::new().unwrap();
    client_runtime.block_on(async {
        let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
        let (mission, _) = client
            .request(http::Method::GET, "/api/missions/gated", None, None)
            .await
            .unwrap();
        assert_eq!(
            mission["mission"]["formations"][0]["brief"]["goal"],
            "Deliver reviewed work"
        );
    });
    click(cx, "prepare-run");
    until(cx, "brief field", |cx| {
        cx.debug_bounds("run-input-brief").is_some()
    });
    click(cx, "run-input-brief");
    cx.simulate_input("hello");
    cx.run_until_parked();
    assert!(
        !shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    );
    click(cx, "run-start");
    until(cx, "active step", |cx| {
        cx.debug_bounds("run-step-fmn_work-running").is_some()
    });
    let run_id = client_runtime.block_on(async {
        let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
        let (runs, _) = client
            .request(http::Method::GET, "/api/runs", None, None)
            .await
            .unwrap();
        runs[0]["runId"].as_str().unwrap().to_owned()
    });
    shell.update(cx, |s, cx| {
        let host = Host::new(s.hub()).for_page("test", &[]);
        host.ask(
            Ask::Reveal {
                what: "run".into(),
                id: run_id.clone(),
                node: Some(NodeId(running.id.clone())),
                part: Some("fmn_work".into()),
            },
            cx,
        )
        .unwrap();
    });
    until(cx, "revealed run step", |cx| {
        cx.debug_bounds("run-reveal-fmn_work").is_some()
    });
    release.send(()).unwrap();
    until(cx, "human gate", |cx| cx.debug_bounds("run-gate").is_some());
    until(cx, "gate request", |cx| {
        cx.debug_bounds("gate-request-loaded").is_some()
    });
    click(cx, "gate-response");
    cx.simulate_input("Approved after review");
    cx.run_until_parked();
    assert!(
        !shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    );
    click(cx, "gate-pass");
    until(cx, "run finished", |cx| {
        cx.debug_bounds("run-step-end_done-done").is_some()
            || cx.debug_bounds("run-finished-succeeded").is_some()
    });
    until(cx, "gate response saved", |cx| {
        shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    });
    shell.update(cx, |s, cx| {
        Host::new(s.hub())
            .for_page("test", &[])
            .ask(
                Ask::Reveal {
                    what: "mission".into(),
                    id: "gated".into(),
                    node: None,
                    part: None,
                },
                cx,
            )
            .unwrap();
    });
    until(cx, "mission after run", |cx| {
        cx.debug_bounds("mission-note-draft").is_some()
    });
    click(cx, "mission-note-draft");
    cx.simulate_input("A note");
    cx.run_until_parked();
    assert!(
        !shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    );
    click(cx, "mission-add-note");
    until(cx, "note saved", |cx| {
        shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    });
    let bad = format!("mission-{}-admission-findings", running.id);
    click(cx, &bad);
    until(cx, "bad mission", |cx| {
        cx.debug_bounds("mission-card-empty").is_some()
    });
    click(cx, "prepare-run");
    until(cx, "start form", |cx| {
        cx.debug_bounds("run-start").is_some()
    });
    let expected = tokio::runtime::Runtime::new().unwrap().block_on(async {
        let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
        let (mission, etag) = client.request(http::Method::GET, "/api/missions/admission-findings", None, None).await.unwrap();
        let error = client.request(http::Method::POST, "/api/runs", Some(json!({"mission":"admission-findings", "inputCardId":"inp", "expectedRev":mission["mission"]["rev"], "inputs":{"topic":"", "sketch":""}})), Some(&etag)).await.unwrap_err();
        error.downcast_ref::<parvi_node_kit::NodeError>().unwrap().findings.as_ref().unwrap().as_array().unwrap().iter().map(|f| format!("{} · {} · {}", f["code"].as_str().unwrap(), f["nodeId"].as_str().unwrap(), f["message"].as_str().unwrap())).collect::<Vec<_>>()
    });
    assert!(expected.len() > 1);
    click(cx, "run-start");
    for (i, finding) in expected.iter().enumerate() {
        let selector = format!("run-finding-{i}-{finding}");
        until(cx, "admission finding", |cx| {
            cx.debug_bounds(Box::leak(selector.clone().into_boxed_str()))
                .is_some()
        });
    }
    let _ = std::fs::remove_dir_all(settings.parent().unwrap());
}
