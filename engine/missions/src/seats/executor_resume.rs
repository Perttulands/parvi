//! Resuming a step whose seat stopped short: the kept seat is asked to go
//! on once per resume, never more, and a seat that is gone says so.
use crate::{
    AgentSeat, Dispatch, DispatchResult, Executor, OpenDispatch, PublishedTurn, SeatEvent,
    SeatExecutor, SeatFuture, SeatId, SeatSide, TurnWorkspace,
};
use parvi_mission_model::{Formation, Slot};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc as stream;

const STOPPED: &str = "seat_idle_without_sentinel: the agent stopped without saying it was done";

/// A seat kept after its first turn stopped short. Each new turn on it takes
/// the next answer; with `gone`, its terminal was closed.
#[derive(Default)]
struct Kept {
    gone: bool,
    answers: Mutex<VecDeque<DispatchResult>>,
    dispatched: Mutex<Vec<Dispatch>>,
    results: Mutex<HashMap<String, DispatchResult>>,
    streams: Mutex<Vec<stream::Sender<Result<SeatEvent, crate::Error>>>>,
}

fn result(status: &str, text: &str) -> DispatchResult {
    DispatchResult {
        status: status.into(),
        text: text.into(),
    }
}

impl SeatSide for Kept {
    fn dispatch(&self, dispatch: Dispatch) -> SeatFuture<'_, String> {
        Box::pin(async move {
            if self.gone {
                return Err(crate::Error::new("seat_error", "no such session"));
            }
            let answer = self.answers.lock().unwrap().pop_front().unwrap();
            self.results
                .lock()
                .unwrap()
                .insert(dispatch.id.clone(), answer);
            self.dispatched.lock().unwrap().push(dispatch.clone());
            Ok(dispatch.id)
        })
    }
    fn events(
        &self,
        _: &str,
        _: u64,
    ) -> Result<stream::Receiver<Result<SeatEvent, crate::Error>>, crate::Error> {
        let (sender, events) = stream::channel(4);
        self.streams.lock().unwrap().push(sender);
        Ok(events)
    }
    fn stage(&self, _: &str, _: &str) -> SeatFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn dispatch_info(&self, id: &str) -> Result<Dispatch, crate::Error> {
        if id == "dsp_initial" {
            return Ok(Dispatch {
                id: id.into(),
                run_id: "run_1".into(),
                node_id: "fmn_answer".into(),
                slot_id: "slot_1".into(),
                name: "forget-1-answer".into(),
                harness: "claude-code".into(),
                model: "haiku".into(),
                effort: "low".into(),
                cwd: PathBuf::from("/w"),
                brief: "Answer.".into(),
                reuse_dispatch: None,
                place: String::new(),
                turn: None,
            });
        }
        self.dispatched
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.id == id)
            .cloned()
            .ok_or_else(|| crate::Error::new("seat_error", "unknown dispatch"))
    }
    fn result(&self, id: &str) -> Result<Option<DispatchResult>, crate::Error> {
        if id == "dsp_initial" {
            return Ok(Some(result("error", STOPPED)));
        }
        Ok(self.results.lock().unwrap().get(id).cloned())
    }
    fn end_seat(&self, _: &SeatId, _: &str) -> SeatFuture<'_, String> {
        Box::pin(async { Ok("ended".into()) })
    }
    fn seats(&self, _: &str) -> Result<Vec<AgentSeat>, crate::Error> {
        Ok(Vec::new())
    }
    fn seat_alive(&self, _: &SeatId) -> SeatFuture<'_, bool> {
        Box::pin(async { Ok(true) })
    }
    fn prepare_turn(&self, _: &str, _: &TurnWorkspace) -> SeatFuture<'_, PathBuf> {
        Box::pin(async { Ok(PathBuf::new()) })
    }
    fn publish_turn(&self, _: &str, _: &TurnWorkspace) -> SeatFuture<'_, PublishedTurn> {
        Box::pin(std::future::pending())
    }
}

fn open() -> OpenDispatch {
    OpenDispatch {
        run_id: "run_1".into(),
        dispatch_id: "dsp_initial".into(),
        node_id: "fmn_answer".into(),
        slot_id: "slot_1".into(),
        attempt: 1,
        formation: Formation {
            id: "fmn_answer".into(),
            kind: "solo".into(),
            slots: vec![Slot {
                id: "slot_1".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
        artifact_dir: PathBuf::new(),
        dispatch_data: Default::default(),
        canceled: None,
        stopping: None,
        final_judge: false,
        limits: Default::default(),
    }
}

fn resume(seats: &Arc<Kept>) -> Result<String, String> {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let executor = SeatExecutor::new(seats.clone(), runtime.handle().clone());
    executor
        .reattach(open())
        .map(|done| done.text)
        .map_err(|error| error.message)
}

#[test]
fn each_resume_asks_the_kept_seat_to_go_on_once() {
    let seats = Arc::new(Kept::default());
    seats.answers.lock().unwrap().extend([
        result("error", STOPPED),
        result("ok", "<<<PARVI-DONE run-id=run_1 status=ok artifact=x>>>"),
    ]);
    // The first resume's turn stops short again: the step blocks again
    // rather than asking on and on.
    let first = resume(&seats).unwrap_err();
    assert!(first.starts_with("seat_idle_without_sentinel"), "{first}");
    assert_eq!(seats.dispatched.lock().unwrap().len(), 1);
    // The next resume steps over that turn and asks once more.
    assert!(resume(&seats).unwrap().contains("PARVI-DONE"));
    let dispatched = seats.dispatched.lock().unwrap();
    assert_eq!(dispatched.len(), 2);
    for turn in dispatched.iter() {
        assert_eq!(turn.reuse_dispatch.as_deref(), Some("dsp_initial"));
        assert!(turn.brief.starts_with("Answer.\n"), "{}", turn.brief);
        assert!(turn.brief.contains("the operator has resumed it"));
    }
    assert_ne!(dispatched[0].id, dispatched[1].id);
}

#[test]
fn a_kept_seat_that_was_closed_says_to_run_the_step_again() {
    let seats = Arc::new(Kept {
        gone: true,
        ..Default::default()
    });
    let error = resume(&seats).unwrap_err();
    assert!(
        error.starts_with("seat_gone:") && error.contains("Run the step again"),
        "{error}"
    );
}
