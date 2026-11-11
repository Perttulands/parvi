//! A step placed on an environment that does not answer (#231): the wait
//! shows on the step, and stopping the run ends it, before the step's seat
//! exists and while it works.
use crate::{
    AgentSeat, BindingsSnapshot, Dispatch, DispatchResult, ExecutionEvent, ExecutionSink, Executor,
    FormationExecution, PublishedTurn, SeatEvent, SeatExecutor, SeatFuture, SeatId, SeatSide,
    TurnWorkspace, executor::PendingEvent,
};
use parvi_mission_model::{Formation, Slot};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool, atomic::Ordering, mpsc},
    time::Duration,
};
use tokio::sync::mpsc as stream;

/// Seats on an environment whose node never answers; with `seated`, it
/// answered until its seat was ready and its brief staged.
#[derive(Default)]
struct Away {
    seated: bool,
    streams: Mutex<Vec<stream::Sender<Result<SeatEvent, crate::Error>>>>,
}

fn never<'a, T: 'a>() -> SeatFuture<'a, T> {
    Box::pin(std::future::pending())
}

impl SeatSide for Away {
    fn dispatch(&self, dispatch: Dispatch) -> SeatFuture<'_, String> {
        if !self.seated {
            return never();
        }
        Box::pin(async move { Ok(dispatch.id) })
    }
    fn events(
        &self,
        _: &str,
        _: u64,
    ) -> Result<stream::Receiver<Result<SeatEvent, crate::Error>>, crate::Error> {
        let (sender, events) = stream::channel(4);
        let seat = SeatId {
            node: "n2".into(),
            session_name: "parvi-run_1".into(),
            session: parvi_term::SessionRef {
                session_id: "$1".into(),
                pane_id: 1,
                generation: parvi_term::Generation {
                    pid: 1,
                    start_time: 1,
                },
            },
        };
        let prepared = json!({"seat":seat,"briefPath":"brief.md"});
        sender
            .try_send(Ok(SeatEvent {
                key: "seat-prepared".into(),
                seq: 1,
                kind: "seat_prepared".into(),
                data: prepared.as_object().cloned().unwrap_or_default(),
            }))
            .unwrap();
        // The stream stays open: the node is away, not finished.
        self.streams.lock().unwrap().push(sender);
        Ok(events)
    }
    fn stage(&self, _: &str, _: &str) -> SeatFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn dispatch_info(&self, _: &str) -> Result<Dispatch, crate::Error> {
        Err(crate::Error::new("seat_error", "never dispatched"))
    }
    fn result(&self, _: &str) -> Result<Option<DispatchResult>, crate::Error> {
        Ok(None)
    }
    fn end_seat(&self, _: &SeatId, _: &str) -> SeatFuture<'_, String> {
        never()
    }
    fn seats(&self, _: &str) -> Result<Vec<AgentSeat>, crate::Error> {
        Ok(Vec::new())
    }
    fn prepare_turn(&self, _: &str, _: &TurnWorkspace) -> SeatFuture<'_, PathBuf> {
        never()
    }
    fn publish_turn(&self, _: &str, _: &TurnWorkspace) -> SeatFuture<'_, PublishedTurn> {
        never()
    }
    fn away(&self, place: &str) -> Option<String> {
        Some(format!(
            "{place} does not answer: no node of {place} is known"
        ))
    }
}

fn execution(turn: Option<TurnWorkspace>, canceled: Arc<AtomicBool>) -> FormationExecution {
    FormationExecution {
        run_id: "run_1".into(),
        formation: Formation {
            id: "fmn_build".into(),
            kind: "solo".into(),
            device: "box/windows".into(),
            slots: vec![Slot {
                id: "slot_1".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
        bindings: BindingsSnapshot {
            schema: 1,
            run_id: "run_1".into(),
            mission_id: String::new(),
            mission_slug: String::new(),
            mission_rev: 1,
            input_card_id: String::new(),
            bindings: Vec::new(),
            gate_bindings: Vec::new(),
        },
        cwd: PathBuf::new(),
        turn,
        artifact_dir: PathBuf::new(),
        context_paths: Vec::new(),
        work: String::new(),
        inputs: Vec::new(),
        input_text: String::new(),
        attempt: 1,
        judge: false,
        pick_attempts: Vec::new(),
        canceled,
        stopping: Arc::new(AtomicBool::new(false)),
        limits: Default::default(),
    }
}

/// Runs a formation against `seats`, and stops it at the event of `kind`.
fn stop_at(
    seats: Away,
    turn: Option<TurnWorkspace>,
    kind: &str,
) -> (Vec<ExecutionEvent>, Result<(), String>) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let executor = SeatExecutor::new(Arc::new(seats), runtime.handle().clone());
    let canceled = Arc::new(AtomicBool::new(false));
    let request = execution(turn, canceled.clone());
    let (sender, received) = mpsc::channel::<PendingEvent>();
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let result = executor.execute_formation(request, ExecutionSink { sender });
        let _ = done.send(result.map(|_| ()));
    });
    let mut events = Vec::new();
    while let Ok(pending) = received.recv_timeout(Duration::from_secs(10)) {
        let _ = pending.ack.send(Ok(()));
        let stop = pending.event.kind == kind;
        events.push(pending.event);
        if stop {
            canceled.store(true, Ordering::Release);
        }
    }
    let result = finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the stop ends the wait");
    (events, result.map_err(|error| error.message))
}

#[test]
fn a_step_waiting_for_its_place_says_so_and_a_stop_ends_the_wait() {
    // Before its turn is prepared there, and before its dispatch is taken.
    for turn in [Some(TurnWorkspace::default()), None] {
        let (events, result) = stop_at(Away::default(), turn, "seat_state");
        let [wait] = events.as_slice() else {
            panic!("one wait, then nothing: {events:?}");
        };
        assert_eq!(
            (wait.kind.as_str(), wait.slot_id.as_str()),
            ("seat_state", "slot_1")
        );
        assert_eq!(
            wait.data["detail"],
            "box/windows does not answer: no node of box/windows is known"
        );
        assert!(wait.data["since"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(result, Err("canceled".into()));
    }
}

#[test]
fn a_stop_leaves_the_seat_of_a_node_that_went_away() {
    let seated = Away {
        seated: true,
        ..Default::default()
    };
    let (events, result) = stop_at(seated, None, "slot_dispatch");
    assert_eq!(events.len(), 1, "{events:?} {result:?}");
    assert_eq!(
        result,
        Err("canceled while box/windows does not answer: no node of box/windows is known; its seat is left as it is".into())
    );
}
