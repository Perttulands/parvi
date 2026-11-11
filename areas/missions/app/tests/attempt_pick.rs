#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::{Ask, Host};
use parvi_core::NodeId;
use parvi_node_kit::{Built, Listing, NodeClient, Setup};
use parvi_testkit::{node::Node, sessions};
use serde_json::json;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

fn build(setup: &Setup) -> anyhow::Result<Built> {
    parvi_missions::build_with_executor(setup, Some(Box::new(parvi_mission_engine::LabExecutor)))
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
                let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, MISSIONS], "")
                    .await
                    .unwrap();
                let dir = node.dir().join("missions/missions");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(
                    dir.join("pick.mission.toml"),
                    include_str!("fixtures/pick.mission.toml"),
                )
                .unwrap();
                ready.send((node.url(), node.id())).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                node.stop().await;
            })
    });
    let (url, id) = wait.recv_timeout(Duration::from_secs(20)).unwrap();
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
    mut condition: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if condition(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let point = cx.debug_bounds(selector).expect(selector).center();
    cx.simulate_click(point, Modifiers::none());
}
#[gpui::test]
fn comparing_and_picking_attempt_records_the_reason(cx: &mut TestAppContext) {
    let running = start();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
    let run_id = rt.block_on(async {
        let (started, _) = client.request(http::Method::POST, "/api/runs", Some(json!({"mission":"pick","inputCardId":"inp_start","expectedRev":1,"inputs":{"brief":"hello"}})), None).await.unwrap();
        let id = started["runId"].as_str().unwrap().to_owned();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut last_seq = 0;
        let mut verdicts = 0;
        loop {
            let (run, _) = client.request(http::Method::GET, &format!("/api/runs/{id}"), None, None).await.unwrap();
            assert!(Instant::now() < deadline, "waiting for pick gate: {run}");
            if let Some(gate) = run["waitingGates"].as_array().and_then(|g| g.first()) {
                if !gate["pick"].is_null() { break; }
                let seq = gate["requestedSeq"].as_u64().unwrap();
                if seq != last_seq {
                    client.request(http::Method::POST, &format!("/api/runs/{id}/gates/gate_review/verdict"), Some(json!({"requestedSeq":seq,"verdict":"fail","reason":"try again"})), None).await.unwrap();
                    last_seq = seq; verdicts += 1;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(verdicts, 2);
        id
    });
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let settings = std::env::temp_dir()
        .join(format!("parvi-attempt-pick-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(900.)));
    shell.update(cx, |shell, cx| {
        Host::new(shell.hub())
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
    // The gate input and frozen mission both arrive asynchronously and can
    // move the compare control. Wait for both before positioning the scroll.
    until(cx, "gate and mission loaded", |cx| {
        cx.debug_bounds("gate-request-loaded").is_some()
            && [
                "run-step-fmn_work-done",
                "run-step-fmn_work-blocked",
                "run-step-fmn_work-running",
                "run-step-fmn_work-pending",
            ]
            .iter()
            .any(|selector| cx.debug_bounds(selector).is_some())
            && cx.debug_bounds("gate-compare").is_some()
    });
    for _ in 0..12 {
        let y = f32::from(cx.debug_bounds("gate-compare").unwrap().center().y);
        if (120. ..800.).contains(&y) {
            break;
        }
        cx.simulate_event(gpui::ScrollEvent {
            position: gpui::point(gpui::px(700.), gpui::px(500.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(
                gpui::px(0.),
                gpui::px((500. - y).clamp(-400., 400.)),
            )),
            modifiers: Default::default(),
            touch_phase: gpui::TouchPhase::Moved,
            source: gpui::ScrollSource::Pointer,
        });
        cx.run_until_parked();
    }
    let y = f32::from(cx.debug_bounds("gate-compare").unwrap().center().y);
    assert!(
        (120. ..800.).contains(&y),
        "compare control is outside viewport: {y}"
    );
    click(cx, "gate-compare");
    until(cx, "attempts window", |cx| {
        cx.debug_bounds("attempts-window").is_some()
    });
    until(cx, "both attempts", |cx| {
        cx.debug_bounds("attempt-card-1").is_some() && cx.debug_bounds("attempt-card-2").is_some()
    });
    click(cx, "attempt-pip-1");
    click(cx, "attempt-pick-reason");
    cx.simulate_input("The first reads better");
    click(cx, "attempts-pick-go");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        let picked = rt.block_on(async {
            let (doc, _) = client
                .request(
                    http::Method::GET,
                    &format!("/api/runs/{run_id}/evidence/attempts"),
                    None,
                    None,
                )
                .await
                .unwrap();
            doc["attempts"]["steps"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|step| step["attempts"].as_array().unwrap().iter())
                .any(|attempt| {
                    attempt["attempt"] == 1
                        && attempt["picked"]["reason"]["text"] == "The first reads better"
                })
        });
        if picked {
            break;
        }
        assert!(Instant::now() < deadline, "pick was not recorded");
        std::thread::sleep(Duration::from_millis(20));
    }
}
