//! Follow a dispatched turn in the harness's native transcript.
use crate::{HarnessAdapter, Pointer, TranscriptRoots, Turn, TurnState, stage_pointer};
use parvi_term::{
    Sessions,
    control::PaneId,
    waiting::harness::{self, Harness},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

/// Inputs shared while watching one live seat.
pub struct TurnWatch<'a> {
    pub engine: &'a dyn Sessions,
    pub pane: PaneId,
    pub kind: Harness,
    pub adapter: Arc<dyn HarnessAdapter>,
    pub roots: &'a TranscriptRoots,
    pub run_id: &'a str,
    pub canceled: &'a AtomicBool,
    /// Reminders already sent to this dispatch, from its record.
    pub reminded: usize,
}
impl TurnWatch<'_> {
    /// The callback durably records the native acknowledgement. It may be
    /// called again after recovery and therefore must be idempotent.
    pub async fn wait(
        &self,
        pointer: &mut Pointer,
        mut consumed: impl FnMut(&Pointer, &Turn) -> Result<(), String>,
        mut state_event: impl FnMut(&str, &str, String) -> Result<(), String>,
    ) -> Result<String, crate::Error> {
        (async {
        let Self {
            engine,
            pane,
            kind,
            adapter,
            roots,
            run_id,
            canceled,
            reminded,
        } = self;
        let pane = *pane;
        let kind = *kind;
        let _updates = engine
            .follow(pane)
            .ok_or("owned seat is unavailable")?
            .updates;
        let mut acknowledged = false;
        let mut transcript_stamp: Option<(u64, Option<SystemTime>)> = None;
        let mut nudges = super::state::Reminders::new(*reminded);
        let mut waits = super::state::WaitTracker::new(Instant::now());
        loop {
            if canceled.load(Ordering::Acquire) {
                return Err("canceled".into());
            }
            let scan_adapter = adapter.clone();
            let scan_roots = (*roots).clone();
            let scan_pointer = pointer.clone();
            let scan_run = (*run_id).to_owned();
            let turn = tokio::task::spawn_blocking(move || {
                scan_adapter.turn(&scan_roots, &scan_pointer, &scan_run)
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
            if turn.consumed && !acknowledged {
                let first_selection = pointer.transcript.is_none();
                pointer.transcript = turn.transcript.clone();
                pointer.session_id = Some(turn.session_id.clone());
                if first_selection
                    && kind == Harness::Codex
                    && let Some(pid) = pointer.pid
                {
                    pointer.known_threads = crate::codex_threads(&roots.codex, pid)
                        .map_err(|error| error.to_string())?;
                }
                consumed(pointer, &turn)?;
                acknowledged = true;
            }
            if let TurnState::Completed { text } = &turn.state {
                if let Some((state, detail, since)) = waits.observe("", Instant::now()) {
                    state_event(state, detail, since)?;
                }
                return Ok(text.clone());
            }
            let state = match &turn.state {
                TurnState::EndedWithoutSentinel => "turn_ended_without_sentinel",
                TurnState::BackgroundWork => "background_work_pending",
                _ => "",
            };
            if let Some((state, detail, since)) = waits.observe(state, Instant::now()) {
                state_event(state, detail, since)?;
            }
            if turn.consumed
                && adapter
                    .conversation_replaced(roots, pointer)
                    .map_err(|e| e.to_string())?
                    == Some(true)
            {
                return Err("seat conversation changed during dispatch".into());
            }
            let mut activity = false;
            if let Some(path) = &pointer.transcript {
                let meta = std::fs::metadata(path)
                    .map_err(|e| format!("selected transcript unavailable: {e}"))?;
                let stamp = (meta.len(), meta.modified().ok());
                if transcript_stamp != Some(stamp) {
                    transcript_stamp = Some(stamp);
                    activity = true;
                }
            }
            let screen = engine.screen(pane);
            if screen.is_none()
                && !engine
                    .list()
                    .iter()
                    .any(|session| session.panes.iter().any(|p| p.0 == pane))
            {
                return Err("the agent's session ended before it finished".into());
            }
            let idle = turn.consumed
                && !matches!(turn.state, TurnState::BackgroundWork)
                && screen
                    .as_ref()
                    .is_some_and(|screen| harness::input_clear(kind, screen));
            let ended = matches!(turn.state, TurnState::EndedWithoutSentinel);
            match nudges.observe(idle, ended, activity, Instant::now()) {
                None => (),
                Some(super::state::Nudge::GiveUp) => {
                    let said = screen.as_ref().map(super::state::screen_words).unwrap_or_default();
                    return Err(format!(
                        "seat_idle_without_sentinel: the agent stopped without saying it was done, even after {} reminders. Open its terminal to see where it stopped, then resume the run.{}",
                        super::state::REMINDERS,
                        if ended || said.is_empty() { String::new() } else { format!(" Its screen says: {said}") }
                    ));
                }
                Some(super::state::Nudge::Remind) => {
                    let instruction = format!(
                        "Your last reply did not end with the done line, so this step is still open. If the work is finished, reply with only the done line; otherwise finish it first. The done line, ARTIFACT replaced by the path or port id you produced and the closing >>> kept: <<<PARVI-DONE run-id={run_id} status=ok artifact=ARTIFACT>>>"
                    );
                    state_event(
                        "seat_idle_recovery",
                        if ended {
                            "the agent ended its turn without the done line; reminding it"
                        } else {
                            "the agent has sat at an empty prompt without the done line; reminding it"
                        },
                        crate::run_storage::now(),
                    )?;
                    stage_pointer(
                        *engine,
                        pane,
                        kind,
                        adapter.as_ref(),
                        &instruction,
                        canceled,
                    )
                    .await.map_err(|error| error.message)?;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
     }).await.map_err(|message: String| crate::Error::new("seat_error", message))
    }
}
