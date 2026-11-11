//! The operator's Host journey, with a fake service manager and controlled readings.
use std::{
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::link::client::LinkState;
use parvi_host_contract::{
    self as wire, Facts, Point, Reading, ReadingFrame, Service, ServiceList, State,
};
use parvi_node_kit::{Area, BoxFuture, Built, Channel, Client, Listing, Setup};
use parvi_testkit::node::Node;
use serde_json::json;
use tokio::sync::mpsc::{self as tokio_mpsc, UnboundedReceiver, UnboundedSender};

#[derive(Default)]
struct Control {
    on: AtomicBool,
    forwards: AtomicUsize,
    discarded: AtomicUsize,
    restarts: AtomicUsize,
    // One app socket: the test owns the sender and its channel takes the receiver.
    sender: Mutex<Option<UnboundedSender<Vec<u8>>>>,
    receiver: Mutex<Option<UnboundedReceiver<Vec<u8>>>>,
}
static CONTROL: OnceLock<Arc<Control>> = OnceLock::new();
const FAKE: Listing = Listing {
    name: "host",
    kinds: &[wire::HOST..=wire::HOST_EVENT],
    commands: &[],
    build,
};
fn build(_: &Setup) -> anyhow::Result<Built> {
    Built::now(FakeArea(CONTROL.get().unwrap().clone()))
}
struct FakeArea(Arc<Control>);
impl Area for FakeArea {
    fn routes(&self) -> Router {
        let control = self.0.clone();
        Router::new().route("/api/services", get(|| async {
            Json(ServiceList { services: vec![Service {
                id: "fixture".into(), name: "Fixture service".into(), state: "running".into(),
                controllable: true, components: vec!["active".into()], ..Service::default()
            }] })
        })).route("/api/services/fixture/restart", post(move || {
            let control = control.clone();
            async move {
                control.restarts.fetch_add(1, Ordering::SeqCst);
                (StatusCode::BAD_REQUEST, Json(json!({"error":{"code":"service_refused", "message":"Restart refused by fake manager"}})))
            }
        }))
    }
    fn channel(&self, _: Client) -> Option<Box<dyn Channel>> {
        let receiver = self
            .0
            .receiver
            .lock()
            .unwrap()
            .take()
            .expect("one app socket");
        Some(Box::new(FakeChannel {
            control: self.0.clone(),
            receiver,
            initial: None,
        }))
    }
}
struct FakeChannel {
    control: Arc<Control>,
    receiver: UnboundedReceiver<Vec<u8>>,
    initial: Option<Vec<u8>>,
}
impl Channel for FakeChannel {
    fn receive(&mut self, frame: Vec<u8>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let [wire::HOST, on] = frame.as_slice() {
                self.control.on.store(*on != 0, Ordering::SeqCst);
                self.initial = (*on != 0).then(snapshot);
            }
        })
    }
    fn next(&mut self) -> BoxFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            if let Some(frame) = self.initial.take() {
                return Some(frame);
            }
            loop {
                let frame = self.receiver.recv().await?;
                if self.control.on.load(Ordering::SeqCst) {
                    self.control.forwards.fetch_add(1, Ordering::SeqCst);
                    return Some(frame);
                }
                self.control.discarded.fetch_add(1, Ordering::SeqCst);
            }
        })
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
fn sample(cpu: f32) -> ReadingFrame {
    let at = now();
    ReadingFrame {
        reading: Reading {
            at,
            cpu: Some(cpu),
            gpu_reason: Some("nvidia-smi failed".into()),
            ..Reading::default()
        },
        point: Point {
            at,
            cpu: Some(cpu),
            ..Point::default()
        },
    }
}
fn frame(kind: u8, body: &impl serde::Serialize) -> Vec<u8> {
    let mut data = vec![kind];
    data.extend(serde_json::to_vec(body).unwrap());
    data
}
fn snapshot() -> Vec<u8> {
    let reading = sample(10.0);
    frame(
        wire::HOST_STATE,
        &State {
            facts: Facts {
                threads: 4,
                ..Facts::default()
            },
            every: 5_000,
            bucket: 60_000,
            history: vec![reading.point],
            latest: Some(reading.reading),
            ..State::default()
        },
    )
}
struct Running {
    url: String,
    node: String,
    control: Arc<Control>,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn start() -> Running {
    let control = Arc::new(Control::default());
    let (tx, rx) = tokio_mpsc::unbounded_channel();
    *control.sender.lock().unwrap() = Some(tx);
    *control.receiver.lock().unwrap() = Some(rx);
    assert!(CONTROL.set(control.clone()).is_ok());
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let node = Node::start(&[FAKE], "").await.unwrap();
                ready.send((node.url(), node.id())).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                node.stop().await;
            });
    });
    let (url, node) = wait.recv().unwrap();
    Running {
        url,
        node,
        control,
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
fn selector(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}
fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let position = cx
        .debug_bounds(selector)
        .expect("the control is visible")
        .center();
    cx.simulate_click(position, Modifiers::none());
}
#[gpui::test]
fn service_restart_refusal_and_host_subscription_journey(cx: &mut TestAppContext) {
    let running = start();
    let dir = parvi_testkit::fresh_dir("parvi-host-journey");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (url, settings) = (running.url.clone(), dir.join("app.json"));
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_host_app::build_host, parvi_host_app::build_services],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    until(cx, "the node link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    until(cx, "initial host subscription", |_| {
        running.control.on.load(Ordering::SeqCst)
    });
    click(cx, "tab-services");
    until(cx, "host channel off", |_| {
        !running.control.on.load(Ordering::SeqCst)
    });
    let state = selector(format!("svc-state-{}-fixture-running", running.node));
    until(cx, "fixture service running", |cx| {
        cx.debug_bounds(state).is_some()
    });
    let restart = selector(format!("svc-restart-{}-fixture", running.node));
    until(cx, "Restart", |cx| cx.debug_bounds(restart).is_some());
    click(cx, restart);
    let failure = selector(format!("svc-failure-{}-fixture", running.node));
    until(cx, "restart refusal displayed", |cx| {
        cx.debug_bounds(failure).is_some()
    });
    assert_eq!(running.control.restarts.load(Ordering::SeqCst), 1);
    assert_eq!(
        shell.read_with(cx, |s, cx| s
            .hub()
            .read(cx)
            .status
            .as_ref()
            .unwrap()
            .text
            .clone()),
        "Restart refused by fake manager"
    );

    click(cx, "tab-host");
    until(cx, "host channel on", |_| {
        running.control.on.load(Ordering::SeqCst)
    });
    let initial = selector(format!("reading-{}-cpu-10%", running.node));
    until(cx, "the initial reading", |cx| {
        cx.debug_bounds(initial).is_some()
    });
    let gpu_reason = selector(format!("meta-{}-gpu-nvidia-smi failed", running.node));
    until(cx, "GPU failure reason", |cx| {
        cx.debug_bounds(gpu_reason).is_some()
    });
    running
        .control
        .sender
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(frame(wire::HOST_READING, &sample(31.0)))
        .unwrap();
    let reading = selector(format!("reading-{}-cpu-31%", running.node));
    until(cx, "the pushed reading", |cx| {
        cx.debug_bounds(reading).is_some()
    });
    assert_eq!(running.control.forwards.load(Ordering::SeqCst), 1);
    click(cx, "tab-services");
    until(cx, "host channel closes", |_| {
        !running.control.on.load(Ordering::SeqCst)
    });
    running
        .control
        .sender
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(frame(wire::HOST_READING, &sample(88.0)))
        .unwrap();
    until(cx, "closed channel discards a reading", |_| {
        running.control.discarded.load(Ordering::SeqCst) == 1
    });
    assert_eq!(running.control.forwards.load(Ordering::SeqCst), 1);
    assert!(
        cx.debug_bounds(selector(format!("reading-{}-cpu-88%", running.node)))
            .is_none()
    );
    let _ = std::fs::remove_dir_all(dir);
}
