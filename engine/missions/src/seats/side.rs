//! Dispatch-addressed boundary used by the coordinator and future node API.
use crate::{AgentSeat, Dispatch, DispatchResult, PublishedTurn, SeatEvent, SeatId, TurnWorkspace};
use std::{future::Future, path::PathBuf, pin::Pin};
use tokio::sync::mpsc;

pub type DispatchId = String;
pub type SeatFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, crate::Error>> + Send + 'a>>;

pub trait SeatSide: Send + Sync + 'static {
    fn dispatch(&self, dispatch: Dispatch) -> SeatFuture<'_, DispatchId>;
    fn events(
        &self,
        id: &str,
        since: u64,
    ) -> Result<mpsc::Receiver<Result<SeatEvent, crate::Error>>, crate::Error>;
    fn stage(&self, id: &str, text: &str) -> SeatFuture<'_, ()>;
    fn dispatch_info(&self, id: &str) -> Result<Dispatch, crate::Error>;
    fn result(&self, id: &str) -> Result<Option<DispatchResult>, crate::Error>;
    fn end_seat(&self, seat: &SeatId, cause: &str) -> SeatFuture<'_, String>;
    fn seats(&self, run_id: &str) -> Result<Vec<AgentSeat>, crate::Error>;
    /// The working directory of a turn placed on `place`, prepared there from
    /// the run branch at the turn's commit.
    fn prepare_turn(&self, place: &str, turn: &TurnWorkspace) -> SeatFuture<'_, PathBuf>;
    /// Commit and publish a prepared turn's work on `place`.
    fn publish_turn(&self, place: &str, turn: &TurnWorkspace) -> SeatFuture<'_, PublishedTurn>;
    /// Why the node of `place` does not answer, while calls to it wait for
    /// it; `None` once it answers, and always for this node's own seats.
    fn away(&self, _place: &str) -> Option<String> {
        None
    }
    /// The tokens a dispatch's seat has spent so far, as a Limit card counts
    /// them, or `None` where this side cannot read its transcript.
    fn tokens(&self, _id: &str) -> Result<Option<i64>, crate::Error> {
        Ok(None)
    }
    /// Paste a line, such as a time limit's warning, into a dispatch's seat.
    fn notify(&self, _id: &str, _text: &str) -> SeatFuture<'_, ()> {
        Box::pin(async {
            Err(crate::Error::new(
                "seat_unavailable",
                "this seat side cannot paste into its seats",
            ))
        })
    }
}
