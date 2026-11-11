#![cfg(unix)]
use gpui::{
    App, Context, IntoElement, Modifiers, ParentElement, TestAppContext, VisualTestContext, Window,
    div,
};
use parvi_app_kit::{Ask, Host, Mounted, Page, ask::Answers, mount};
use parvi_core::NodeId;
use parvi_mission_engine::{
    ExecutionEvent, ExecutionSink, Executor, FormationExecution, FormationResult, LabExecutor,
};
use parvi_node_kit::{Built, Listing, Setup};
use parvi_testkit::{
    node::Node,
    sessions::{self, FakeSessions},
};
use serde_json::json;
use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

static OPENED: Mutex<Vec<Ask>> = Mutex::new(Vec::new());
static SESSIONS: Mutex<Option<Arc<FakeSessions>>> = Mutex::new(None);
static NODE: Mutex<String> = Mutex::new(String::new());
static RELEASE: Mutex<Option<mpsc::Receiver<()>>> = Mutex::new(None);
struct ToyTerminal;
impl gpui::Render for ToyTerminal {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child("Terminal")
    }
}
impl Page for ToyTerminal {
    const ID: &'static str = "toy-terminal";
    const TITLE: &'static str = "Terminal";
    const ANSWERS: &'static [Answers] = &[Answers::Reveal("session")];
    fn answer(&mut self, ask: Ask, _: &'static str, _: &mut Window, _: &mut Context<Self>) {
        OPENED.lock().unwrap().push(ask);
    }
}
fn terminal(host: Host, window: &mut Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, |_, _, _| ToyTerminal)
}
struct SeatLab;
impl Executor for SeatLab {
    fn execute_formation(
        &self,
        request: FormationExecution,
        sink: ExecutionSink,
    ) -> Result<FormationResult, parvi_core::Error> {
        let sessions = SESSIONS.lock().unwrap().clone().unwrap();
        let name = format!("seat-{}", request.formation.id);
        let session = sessions.add(&name);
        let slot = request.formation.slots[0].id.clone();
        let node = NODE.lock().unwrap().clone();
        sink.emit(ExecutionEvent {
            kind: "seat_created".into(),
            slot_id: slot.clone(),
            data:
                json!({"node":node,"sessionName":name,"sessionId":session,"dispatchId":"lab-seat"})
                    .as_object()
                    .unwrap()
                    .clone(),
        })?;
        let receiver = RELEASE.lock().unwrap().take().unwrap();
        receiver
            .recv_timeout(Duration::from_secs(120))
            .map_err(|e| parvi_core::Error::new("test", e.to_string()))?;
        let result = LabExecutor.execute_formation(request, sink.clone())?;
        sink.emit(ExecutionEvent {
            kind: "seat_cleanup".into(),
            slot_id: slot,
            data: json!({"sessionName":name,"outcome":"ended","cause":"run_final"})
                .as_object()
                .unwrap()
                .clone(),
        })?;
        Ok(result)
    }
}
fn build(setup: &Setup) -> anyhow::Result<Built> {
    parvi_missions::build_with_executor(setup, Some(Box::new(SeatLab)))
}
const MISSIONS: Listing = Listing {
    build,
    ..parvi_missions::LISTING
};
struct Running {
    url: String,
    node: String,
    run: String,
    session: String,
    seq: u64,
    release: Option<mpsc::Sender<()>>,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
fn start() -> Running {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let (release, receiver) = mpsc::channel();
    *RELEASE.lock().unwrap() = Some(receiver);
    let thread = std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
        let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, MISSIONS], "").await.unwrap();
        *SESSIONS.lock().unwrap() = Some(node.fake_sessions());
        *NODE.lock().unwrap() = node.id();
        let dir = node.dir().join("missions/missions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hello.mission.toml"), include_str!("fixtures/solo-lab.mission.toml")).unwrap();
        let start = node.call("mission_run", json!({"mission":"hello","input":"inp_hello","inputs":{"brief":"ship"},"cwd":node.dir()})).await.unwrap();
        let run = start["runId"].as_str().unwrap().to_owned();
        let deadline = Instant::now() + Duration::from_secs(15);
        let (seq, session) = loop {
            let status = node.call("run_status", json!({"run":run})).await.unwrap();
            if let Some(event) = status["events"].as_array().unwrap().iter().find(|event| event["type"] == "seat_created") {
                break (event["seq"].as_u64().unwrap(), event["sessionId"].as_str().unwrap().to_owned());
            }
            assert!(Instant::now() < deadline, "seat not recorded: {status}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        ready.send((node.url(), node.id(), run, session, seq)).unwrap();
        while stopped.try_recv().is_err() { tokio::time::sleep(Duration::from_millis(20)).await; }
        node.stop().await;
        *SESSIONS.lock().unwrap() = None;
    })
    });
    let (url, node, run, session, seq) = wait.recv_timeout(Duration::from_secs(20)).unwrap();
    Running {
        url,
        node,
        run,
        session,
        seq,
        release: Some(release),
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
#[gpui::test]
fn step_and_run_bar_open_the_owned_session_and_disable_ended_seats(cx: &mut TestAppContext) {
    OPENED.lock().unwrap().clear();
    let mut running = start();
    cx.executor().allow_parking();
    cx.update(|cx| {
        parvi_app_shell::prepare(cx);
        cx.set_app_identity("io.parvi.seat-test", "Parvi seat test");
    });
    let settings = std::env::temp_dir()
        .join(format!("parvi-seat-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build, terminal],
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
                    node: Some(NodeId(running.node.clone())),
                    part: None,
                    beside: false,
                },
                cx,
            )
            .unwrap()
    });
    until(cx, "mission", |cx| {
        cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    click(cx, "mission-card-fmn_work");
    let step = format!(
        "step-seat-{}-{}-live-seat-fmn_work",
        running.run, running.seq
    );
    until(cx, "live step seat", |cx| {
        cx.debug_bounds(Box::leak(step.clone().into_boxed_str()))
            .is_some()
    });
    click(cx, &step);
    until(cx, "step terminal ask", |_| {
        !OPENED.lock().unwrap().is_empty()
    });
    let expected = Ask::Reveal {
        what: "session".into(),
        id: running.session.clone(),
        node: Some(NodeId(running.node.clone())),
        part: None,
        beside: false,
    };
    assert_eq!(OPENED.lock().unwrap().last().unwrap(), &expected);
    shell.update(cx, |shell, cx| {
        Host::new(shell.hub())
            .for_page("test", &[])
            .ask(
                Ask::Reveal {
                    what: "run".into(),
                    id: running.run.clone(),
                    node: Some(NodeId(running.node.clone())),
                    part: None,
                    beside: false,
                },
                cx,
            )
            .unwrap()
    });
    let bar = format!(
        "run-bar-seat-{}-{}-live-seat-fmn_work",
        running.run, running.seq
    );
    until(cx, "live run seat", |cx| {
        cx.debug_bounds(Box::leak(bar.clone().into_boxed_str()))
            .is_some()
    });
    click(cx, &bar);
    until(cx, "bar terminal ask", |_| {
        OPENED.lock().unwrap().len() == 2
    });
    assert_eq!(OPENED.lock().unwrap().last().unwrap(), &expected);
    running.release.take().unwrap().send(()).unwrap();
    shell.update(cx, |shell, cx| {
        Host::new(shell.hub())
            .for_page("test", &[])
            .ask(
                Ask::Reveal {
                    what: "run".into(),
                    id: running.run.clone(),
                    node: Some(NodeId(running.node.clone())),
                    part: None,
                    beside: false,
                },
                cx,
            )
            .unwrap();
    });
    let ended = format!(
        "run-bar-seat-{}-{}-ended-seat-fmn_work",
        running.run, running.seq
    );
    until(cx, "ended seat", |cx| {
        cx.debug_bounds(Box::leak(ended.clone().into_boxed_str()))
            .is_some()
    });
    click(cx, &ended);
    cx.run_until_parked();
    assert_eq!(
        OPENED.lock().unwrap().len(),
        2,
        "ended seat must not open a terminal"
    );
    shell.update(cx, |shell, cx| {
        Host::new(shell.hub())
            .for_page("test", &[])
            .ask(
                Ask::Reveal {
                    what: "mission".into(),
                    id: "hello".into(),
                    node: Some(NodeId(running.node.clone())),
                    part: None,
                    beside: false,
                },
                cx,
            )
            .unwrap();
    });
    until(cx, "mission after run", |cx| {
        cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    click(cx, "mission-card-fmn_work");
    let step_ended = format!(
        "step-seat-{}-{}-ended-seat-fmn_work",
        running.run, running.seq
    );
    until(cx, "ended step seat", |cx| {
        cx.debug_bounds(Box::leak(step_ended.clone().into_boxed_str()))
            .is_some()
    });
    click(cx, &step_ended);
    cx.run_until_parked();
    assert_eq!(
        OPENED.lock().unwrap().len(),
        2,
        "ended step seat must not open a terminal"
    );
}
