#![cfg(unix)]
use gpui::ParentElement;
use gpui::{
    App, Context, IntoElement, Modifiers, SystemNotificationResponse, TestAppContext,
    VisualTestContext, Window, point, px,
};
use parvi_app_kit::{Ask, Host, Mounted, Page, link::client::LinkState, mount};
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

static FAIL_GATE_READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WATCHES: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
struct ObservedArea(Arc<dyn parvi_node_kit::Area>);
struct ObservedChannel(Box<dyn parvi_node_kit::Channel>);
impl parvi_node_kit::Channel for ObservedChannel {
    fn receive(&mut self, frame: Vec<u8>) -> parvi_node_kit::BoxFuture<'_, ()> {
        WATCHES.lock().unwrap().push(frame.clone());
        self.0.receive(frame)
    }
    fn next(&mut self) -> parvi_node_kit::BoxFuture<'_, Option<Vec<u8>>> {
        self.0.next()
    }
}
impl parvi_node_kit::Area for ObservedArea {
    fn routes(&self) -> axum::Router {
        self.0.routes().layer(axum::middleware::from_fn(
            |request: axum::extract::Request, next: axum::middleware::Next| async move {
                if request.uri().path().contains("/gates/")
                    && request.uri().path().ends_with("/request")
                    && FAIL_GATE_READ.load(std::sync::atomic::Ordering::SeqCst)
                {
                    return axum::response::IntoResponse::into_response((
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        "test gate read failure",
                    ));
                }
                next.run(request).await
            },
        ))
    }
    fn commands(&self) -> Vec<parvi_node_kit::Command> {
        self.0.commands()
    }
    fn channel(&self, client: parvi_node_kit::Client) -> Option<Box<dyn parvi_node_kit::Channel>> {
        self.0
            .channel(client)
            .map(|channel| Box::new(ObservedChannel(channel)) as Box<dyn parvi_node_kit::Channel>)
    }
    fn stop(&self) -> parvi_node_kit::BoxFuture<'_, ()> {
        self.0.stop()
    }
}
struct Other;
impl gpui::Render for Other {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::div().child("Other page")
    }
}
impl Page for Other {
    const ID: &'static str = "other";
    const TITLE: &'static str = "Other";
}
fn other(host: Host, window: &mut Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, |_, _, _| Other)
}

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
    let built = parvi_missions::build_with_executor(
        setup,
        Some(Box::new(FakeSeatLab {
            runtime: tokio::runtime::Handle::current(),
        })),
    )?;
    Ok(match built {
        Built::Now(area) => Built::Now(Arc::new(ObservedArea(area))),
        Built::Then(finish) => Built::Then(Box::new(move |provided| {
            Ok(Arc::new(ObservedArea(finish(provided)?)))
        })),
    })
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
    cx.update(|cx| {
        parvi_app_shell::prepare(cx);
        cx.set_app_identity("io.parvi.test", "Parvi test");
    });
    let (url, path) = (running.url.clone(), settings.clone());
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build, other],
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
                beside: false,
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
    until(cx, "mission flow", |cx| {
        cx.debug_bounds("flow-step-fmn_work").is_some()
    });
    click(cx, "mission-mode-canvas");
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
    click(cx, "prepare-run");
    cx.run_until_parked();
    assert!(cx.debug_bounds("run-input-brief").is_none());
    assert!(cx.debug_bounds("step-goal").is_some());
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
                    beside: false,
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
    click(cx, "run-cancel");
    until(cx, "start cancelled", |cx| {
        cx.debug_bounds("run-input-brief").is_none()
    });
    click(cx, "prepare-run");
    until(cx, "brief reopened", |cx| {
        cx.debug_bounds("run-input-brief").is_some()
    });
    cx.simulate_resize(gpui::size(px(1600.), px(1000.)));
    cx.run_until_parked();
    let footer = cx.debug_bounds("run-start-actions").unwrap();
    let start = cx.debug_bounds("run-start").unwrap();
    let cancel = cx.debug_bounds("run-cancel").unwrap();
    assert!(start.center().y >= footer.origin.y && cancel.center().y >= footer.origin.y);
    let paths = cx.debug_bounds("run-context-paths").unwrap();
    assert!(paths.origin.y + paths.size.height <= footer.origin.y);
    cx.simulate_resize(gpui::size(px(800.), px(600.)));
    cx.run_until_parked();
    let footer = cx.debug_bounds("run-start-actions").unwrap();
    assert!(footer.origin.y + footer.size.height <= px(600.));
    cx.simulate_resize(gpui::size(px(1600.), px(1000.)));
    cx.run_until_parked();
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
            && cx.debug_bounds("run-sentence-running Work").is_some()
    });
    for _ in 0..8 {
        let y = f32::from(cx.debug_bounds("run-abort").unwrap().center().y);
        if (100. ..800.).contains(&y) {
            break;
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
    click(cx, "run-abort");
    until(cx, "stop confirmation", |cx| {
        cx.debug_bounds("stop-run").is_some()
    });
    click(cx, "stop-reason");
    cx.simulate_input("Checking the stop dialog");
    click(cx, "stop-keep");
    until(cx, "stop dismissed", |cx| {
        cx.debug_bounds("stop-run").is_none()
    });
    for _ in 0..8 {
        let y = f32::from(cx.debug_bounds("run-mode-canvas").unwrap().center().y);
        if (100. ..800.).contains(&y) {
            break;
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
    click(cx, "run-mode-canvas");
    until(cx, "run canvas", |cx| {
        cx.debug_bounds("mission-canvas").is_some()
    });
    click(cx, "run-mode-flow");
    until(cx, "run flow", |cx| {
        cx.debug_bounds("run-step-fmn_work-running").is_some()
    });
    click(cx, "run-events");
    until(cx, "events drawer", |cx| {
        cx.debug_bounds("run-events-drawer").is_some()
    });
    click(cx, "run-events");
    until(cx, "events closed", |cx| {
        cx.debug_bounds("run-events-drawer").is_none()
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
                beside: false,
            },
            cx,
        )
        .unwrap();
    });
    until(cx, "revealed run step", |cx| {
        cx.debug_bounds("run-reveal-fmn_work").is_some()
    });
    click(cx, "tab-other");
    until(
        cx,
        "mission list channel off while run channel stays on",
        |_| {
            let frames = WATCHES.lock().unwrap();
            frames
                .iter()
                .any(|frame| frame.as_slice() == [parvi_missions_contract::WATCH, 0])
                && frames
                    .iter()
                    .any(|frame| frame.as_slice() == [parvi_missions_contract::WATCH_RUNS, 1])
        },
    );
    assert!(
        !WATCHES
            .lock()
            .unwrap()
            .iter()
            .any(|frame| frame.as_slice() == [parvi_missions_contract::WATCH_RUNS, 0])
    );
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    cx.deactivate_window();
    assert!(!cx.update(|window, _| window.is_window_active()));
    FAIL_GATE_READ.store(true, std::sync::atomic::Ordering::SeqCst);
    release.send(()).unwrap();
    until(cx, "hidden missions count", |cx| {
        cx.debug_bounds("missions-count-1").is_some()
    });
    assert!(cx.debug_bounds("tab-label-M I S S I O N S").is_some());
    until(cx, "desktop notification", |cx| {
        !cx.shown_system_notifications().is_empty()
    });
    assert_eq!(cx.shown_system_notifications().len(), 1);
    let tag = cx.shown_system_notifications()[0].tag.clone();
    cx.simulate_system_notification_response(SystemNotificationResponse {
        tag,
        action_id: None,
    });
    until(cx, "needs-you group", |cx| {
        cx.debug_bounds("needs-you-group").is_some()
    });
    let group = cx.debug_bounds("needs-you-group").unwrap();
    let row_selector = Box::leak(format!("run-row-{run_id}").into_boxed_str());
    let row = cx.debug_bounds(row_selector).unwrap();
    assert!(row.origin.y > group.origin.y);
    let mission_selector = Box::leak(format!("mission-{}-gated", running.id).into_boxed_str());
    let mission = cx.debug_bounds(mission_selector).unwrap();
    assert!(row.origin.y < mission.origin.y);
    until(cx, "human gate", |cx| cx.debug_bounds("run-gate").is_some());
    assert!(cx.debug_bounds("run-mission-Gated").is_some());
    assert!(cx.debug_bounds("gate-title-Review").is_some());
    until(cx, "waiting sentence", |cx| {
        cx.debug_bounds("run-sentence-waiting for you at Review")
            .is_some()
    });
    until(cx, "gate read failure", |cx| {
        cx.debug_bounds("gate-retry").is_some()
    });
    click(cx, "gate-response");
    cx.simulate_input("Approved after review");
    FAIL_GATE_READ.store(false, std::sync::atomic::Ordering::SeqCst);
    click(cx, "gate-retry");
    until(cx, "gate request", |cx| {
        cx.debug_bounds("gate-request-loaded").is_some()
    });
    assert!(cx.debug_bounds("gate-source-Work").is_some());
    assert!(
        cx.debug_bounds("gate-question-Review the result and decide whether to approve it.")
            .is_some()
    );
    assert!(cx.debug_bounds("gate-input-quote").is_some());
    cx.run_until_parked();
    assert!(
        !shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    );
    click(cx, "gate-pass");
    until(cx, "count cleared", |cx| {
        cx.debug_bounds("missions-count-1").is_none()
    });
    assert_eq!(cx.shown_system_notifications().len(), 1);
    until(cx, "run finished", |cx| {
        cx.debug_bounds("run-step-end_done-done").is_some()
            || cx.debug_bounds("run-finished-succeeded").is_some()
    });
    assert!(cx.debug_bounds("run-land").is_none());
    assert!(cx.debug_bounds("run-send").is_none());
    click(cx, "run-events");
    until(cx, "gate response in run history", |cx| {
        cx.debug_bounds("run-event-human_verdict_recorded-Approved after review")
            .is_some()
    });
    until(cx, "gate response saved", |cx| {
        shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty()
    });
    click(cx, "run-again");
    until(cx, "start form for the same mission", |cx| {
        cx.debug_bounds("run-start").is_some()
    });
    assert!(
        !shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty(),
        "Run again carries the finished run's inputs into the start form"
    );
    click(cx, "run-cancel");
    until(cx, "start form closed", |cx| {
        cx.debug_bounds("run-start").is_none()
    });
    shell.update(cx, |s, cx| {
        Host::new(s.hub())
            .for_page("test", &[])
            .ask(
                Ask::Reveal {
                    what: "run".into(),
                    id: run_id.clone(),
                    node: Some(NodeId(running.id.clone())),
                    part: None,
                    beside: false,
                },
                cx,
            )
            .unwrap();
    });
    until(cx, "the run's output", |cx| {
        cx.debug_bounds("use-output-result").is_some()
    });
    click(cx, "use-output-result");
    until(cx, "missions to use it in", |cx| {
        cx.debug_bounds("use-in-gated").is_some()
    });
    click(cx, "use-in-gated");
    until(cx, "start form holding the output", |cx| {
        cx.debug_bounds("run-start").is_some()
    });
    assert!(
        !shell
            .read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx))
            .is_empty(),
        "the output fills the next mission's first input"
    );
    click(cx, "run-cancel");
    until(cx, "start form closed again", |cx| {
        cx.debug_bounds("run-start").is_none()
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
                    beside: false,
                },
                cx,
            )
            .unwrap();
    });
    until(cx, "mission after run", |cx| {
        cx.debug_bounds("mission-mode-flow").is_some()
    });
    click(cx, "mission-mode-flow");
    until(cx, "mission notes in Flow", |cx| {
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
    click(cx, "mission-mode-canvas");
    let bad = format!("mission-{}-admission-findings", running.id);
    click(cx, &bad);
    until(cx, "bad mission", |cx| {
        cx.debug_bounds("mission-card-empty").is_some()
    });
    click(cx, "prepare-run");
    until(cx, "start form", |cx| {
        cx.debug_bounds("run-start").is_some()
    });
    click(cx, "run-start");
    until(cx, "required input", |cx| {
        cx.debug_bounds("run-required-topic").is_some()
    });
    click(cx, "run-input-topic");
    cx.simulate_input("A topic");
    let expected = tokio::runtime::Runtime::new().unwrap().block_on(async {
        let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
        let (mission, etag) = client.request(http::Method::GET, "/api/missions/admission-findings", None, None).await.unwrap();
        let error = client.request(http::Method::POST, "/api/runs", Some(json!({"mission":"admission-findings", "inputCardId":"inp", "expectedRev":mission["mission"]["rev"], "inputs":{"topic":"A topic"}})), Some(&etag)).await.unwrap_err();
        // Each finding reads as a sentence, named by its step's title.
        let title = |id: &str| mission["mission"]["formations"].as_array().unwrap().iter().find(|f| f["id"] == id).map_or(id.to_owned(), |f| f["title"].as_str().unwrap().to_owned());
        error.downcast_ref::<parvi_node_kit::NodeError>().unwrap().findings.as_ref().unwrap().as_array().unwrap().iter().map(|f| match f["nodeId"].as_str().unwrap() { "" => f["message"].as_str().unwrap().to_owned(), id => format!("{}: {}", title(id), f["message"].as_str().unwrap()) }).collect::<Vec<_>>()
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
