//! A lab seat side makes real dispatch records and uses testkit's fake Sessions.
use parvi_mission_engine::{
    AgentSeat, Coordinator, Dispatch, DispatchResult, PublishedTurn, RunSettings, SeatEvent,
    SeatExecutor, SeatFuture, SeatId, SeatSide, TurnWorkspace,
};
use parvi_mission_model::RunStart;
use parvi_term::{Sessions, Start};
use parvi_testkit::{node::Node, sessions};
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

#[derive(Clone)]
struct LabSeats {
    sessions: Arc<dyn Sessions>,
    root: PathBuf,
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}
struct Entry {
    dispatch: Dispatch,
    seat: SeatId,
    path: PathBuf,
    events: Vec<SeatEvent>,
    staged: bool,
}
impl SeatSide for LabSeats {
    fn dispatch(&self, request: Dispatch) -> SeatFuture<'_, String> {
        Box::pin(async move {
            let id = self
                .sessions
                .start(Start {
                    name: format!("seat-{}", request.slot_id),
                    folder: request.cwd.to_string_lossy().into(),
                    command: String::new(),
                    user: String::new(),
                })
                .await
                .map_err(|e| parvi_mission_engine::Error::new("fake", e.to_string()))?;
            let pane = self
                .sessions
                .active_pane(&id)
                .await
                .map_err(|e| parvi_mission_engine::Error::new("fake", e.to_string()))?;
            let seat = SeatId {
                node: "test-node".into(),
                session_name: format!("seat-{}", request.slot_id),
                session: parvi_term::SessionRef {
                    session_id: id,
                    pane_id: pane,
                    generation: parvi_term::Generation {
                        pid: 1,
                        start_time: 1,
                    },
                },
            };
            let path = self.root.join(format!("{}.md", request.slot_id));
            std::fs::write(&path, &request.brief)
                .map_err(|e| parvi_mission_engine::Error::new("fake", e.to_string()))?;
            let created = json!({"dispatchId":request.id,"node":seat.node,"sessionName":seat.session_name,"sessionId":seat.session.session_id,"paneId":seat.session.pane_id,"generation":seat.session.generation});
            let prepared = json!({"seat":seat,"briefPath":path});
            let events = vec![
                SeatEvent {
                    key: "created".into(),
                    seq: 1,
                    kind: "seat_created".into(),
                    data: created.as_object().unwrap().clone(),
                },
                SeatEvent {
                    key: "prepared".into(),
                    seq: 2,
                    kind: "seat_prepared".into(),
                    data: prepared.as_object().unwrap().clone(),
                },
            ];
            let dispatch_id = request.id.clone();
            self.entries.lock().unwrap().insert(
                dispatch_id.clone(),
                Entry {
                    dispatch: request,
                    seat,
                    path,
                    events,
                    staged: false,
                },
            );
            Ok(dispatch_id)
        })
    }
    fn events(
        &self,
        id: &str,
        since: u64,
    ) -> Result<
        mpsc::Receiver<Result<SeatEvent, parvi_mission_engine::Error>>,
        parvi_mission_engine::Error,
    > {
        let entries = self.entries.clone();
        let id = id.to_owned();
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            let mut cursor = since;
            loop {
                let events = entries.lock().unwrap().get(&id).unwrap().events.clone();
                for event in events
                    .into_iter()
                    .filter(|event| event.seq > cursor)
                    .collect::<Vec<_>>()
                {
                    cursor = event.seq;
                    if tx.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
                tokio::select! { _ = tx.closed() => return, _ = tokio::time::sleep(Duration::from_millis(10)) => {} }
            }
        });
        Ok(rx)
    }
    fn stage(&self, id: &str, _: &str) -> SeatFuture<'_, ()> {
        let id = id.to_owned();
        Box::pin(async move {
            self.entries.lock().unwrap().get_mut(&id).unwrap().staged = true;
            Ok(())
        })
    }
    fn dispatch_info(&self, id: &str) -> Result<Dispatch, parvi_mission_engine::Error> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(id)
            .unwrap()
            .dispatch
            .clone())
    }
    fn result(&self, _: &str) -> Result<Option<DispatchResult>, parvi_mission_engine::Error> {
        Ok(None)
    }
    fn end_seat(&self, seat: &SeatId, cause: &str) -> SeatFuture<'_, String> {
        let seat = seat.clone();
        let cause = cause.to_owned();
        Box::pin(async move {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries
                .values_mut()
                .find(|entry| entry.seat == seat)
                .unwrap();
            let data = json!({"sessionName":seat.session_name,"outcome":if cause == "shutdown" {"left_shutdown"} else {"ended"},"cause":cause});
            entry.events.push(SeatEvent {
                key: "cleanup".into(),
                seq: 3,
                kind: "seat_cleanup".into(),
                data: data.as_object().unwrap().clone(),
            });
            Ok("ended".into())
        })
    }
    fn seats(&self, _: &str) -> Result<Vec<AgentSeat>, parvi_mission_engine::Error> {
        Ok(vec![])
    }
    fn prepare_turn(&self, _: &str, _: &TurnWorkspace) -> SeatFuture<'_, PathBuf> {
        Box::pin(async { unreachable!() })
    }
    fn publish_turn(&self, _: &str, _: &TurnWorkspace) -> SeatFuture<'_, PublishedTurn> {
        Box::pin(async { unreachable!() })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn controller_receives_worker_sessions_and_team_is_recorded() {
    let node = Node::start(&[sessions::LISTING], "").await.unwrap();
    let root = parvi_testkit::fresh_dir("orchestrated-seats");
    std::fs::create_dir_all(root.join("missions")).unwrap();
    let mission = include_str!("parity/runs/missions/solo-lab.mission.toml")
        .replace("type = \"solo\"", "type = \"orchestrated\"")
        .replace("id = \"slot_work\"", "id = \"slot_lead\"\ncontroller = true")
        .replace("[[end]]", "[[formation.slot]]\nid = \"slot_a\"\nlabel = \"Worker A\"\nharness = \"openai-codex\"\neffort = \"medium\"\n[[formation.slot]]\nid = \"slot_b\"\nlabel = \"Worker B\"\nharness = \"claude-code\"\neffort = \"medium\"\n\n[[end]]");
    std::fs::write(root.join("missions/hello.mission.toml"), mission).unwrap();
    let seats = LabSeats {
        sessions: node.sessions(),
        root: root.clone(),
        entries: Default::default(),
    };
    let coordinator = Coordinator::open(
        &root,
        RunSettings::default(),
        Box::new(SeatExecutor::new(
            Arc::new(seats.clone()),
            tokio::runtime::Handle::current(),
        )),
    )
    .unwrap();
    let run = coordinator
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            cwd: root.to_string_lossy().into(),
            inputs: BTreeMap::from([("brief".into(), "ship".into())]),
            ..Default::default()
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let projection = coordinator.run(&run).unwrap();
            if projection
                .events
                .iter()
                .any(|event| event.kind == "slot_dispatch" && event.slot_id == "slot_lead")
                && seats
                    .entries
                    .lock()
                    .unwrap()
                    .values()
                    .any(|entry| entry.dispatch.slot_id == "slot_lead" && entry.staged)
            {
                break;
            }
            assert!(
                !projection.r#final && projection.status != "blocked",
                "{}",
                projection.status
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("controller dispatched");
    let sessions = node.sessions().list();
    assert_eq!(sessions.len(), 3);
    {
        let entries = seats.entries.lock().unwrap();
        let lead = entries
            .values()
            .find(|entry| entry.dispatch.slot_id == "slot_lead")
            .unwrap();
        let brief = std::fs::read_to_string(&lead.path).unwrap();
        assert!(lead.staged);
        let attempt = brief
            .lines()
            .find(|line| line.starts_with("Attempt directory: "))
            .unwrap();
        for worker in ["slot_a", "slot_b"] {
            let entry = entries
                .values()
                .find(|entry| entry.dispatch.slot_id == worker)
                .unwrap();
            assert!(
                !entry.staged,
                "the controller delegates the worker's first task"
            );
            assert!(brief.contains(&entry.seat.session_name));
            assert!(brief.contains(&format!(
                "parvi session send {} --text",
                entry.seat.session_name
            )));
            assert!(brief.contains(entry.path.to_str().unwrap()));
            assert!(
                std::fs::read_to_string(&entry.path)
                    .unwrap()
                    .lines()
                    .any(|line| line == attempt)
            );
        }
    }
    let ledger =
        parvi_mission_engine::Ledger::new(root.join("runs").join(format!("{run}.ndjson")), &run)
            .read()
            .unwrap();
    let team = ledger
        .iter()
        .find(|event| event.kind == "orchestration_team")
        .unwrap();
    assert_eq!(team.data["controllerSlot"], "slot_lead");
    assert_eq!(team.data["workers"].as_array().unwrap().len(), 2);
    assert_eq!(
        ledger
            .iter()
            .filter(|event| event.kind == "worker_observation")
            .count(),
        2
    );
    let c = coordinator.clone();
    tokio::task::spawn_blocking(move || c.abort(&run, "human:test", "test ends"))
        .await
        .unwrap()
        .unwrap();
    drop(coordinator);
    node.stop().await;
    std::fs::remove_dir_all(root).unwrap();
}
