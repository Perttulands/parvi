//! Safe pointer delivery through the terminal engine's live screen.
use crate::{HarnessAdapter, Submission, SubmissionAction};
use parvi_term::{
    Sessions,
    control::PaneId,
    waiting::harness::{self, Harness},
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Paste a pointer only into an idle, empty editor and submit it only while
/// that editor contains exactly the pointer. Screen updates wake the loop;
/// the timer permits bounded Enter retries when a harness redraws nothing.
pub async fn stage_pointer(
    engine: &dyn Sessions,
    pane: PaneId,
    kind: Harness,
    adapter: &dyn HarnessAdapter,
    pointer: &str,
    canceled: &AtomicBool,
) -> Result<(), crate::Error> {
    (async {
        stage_guarded(
            engine,
            pane,
            kind,
            adapter,
            pointer,
            canceled,
            false,
            false,
            || Ok(()),
            |_, _, _| Ok(()),
        )
        .await
    })
    .await
    .map_err(|message: String| crate::Error::new("seat_error", message))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn stage_guarded(
    engine: &dyn Sessions,
    pane: PaneId,
    kind: Harness,
    adapter: &dyn HarnessAdapter,
    pointer: &str,
    canceled: &AtomicBool,
    recovering: bool,
    resume_enter: bool,
    mut before_enter: impl FnMut() -> Result<(), String>,
    mut state_event: impl FnMut(&str, &str, String) -> Result<(), String>,
) -> Result<(), String> {
    if harness::rendered(pointer).is_empty() {
        return Err("empty brief pointer".into());
    }
    let mut updates = engine
        .follow(pane)
        .ok_or("owned seat is unavailable")?
        .updates;
    let mut staged = false;
    let mut submission = Submission::new(pointer);
    let mut trust_finished = false;
    let mut trust_entered = false;
    let mut last_trust_key: Option<(String, Instant)> = None;
    let mut trust_presses = 0;
    let mut seen_ready = false;
    let mut waits = super::state::WaitTracker::new(Instant::now());
    loop {
        if canceled.load(Ordering::Acquire) {
            return Err("canceled".into());
        }
        let screen = engine.screen(pane);
        if let Some(screen) = screen {
            seen_ready |= adapter.ready(&screen);
            let dialog = if !trust_finished && harness::input(kind, &screen).is_none() {
                adapter.trust_dialog(&screen)
            } else {
                None
            };
            if let Some(keys) = dialog {
                let due = last_trust_key.as_ref().is_none_or(|(last, at)| {
                    *last != keys.0 || at.elapsed() >= Duration::from_millis(1500)
                });
                if due && trust_presses < 5 {
                    let bytes = match keys.0.as_str() {
                        "Down" => b"\x1b[B".as_slice(),
                        "Enter" => {
                            trust_entered = true;
                            b"\r".as_slice()
                        }
                        _ => return Err("unknown trust dialog key".into()),
                    };
                    engine
                        .input(pane, bytes.to_vec())
                        .await
                        .map_err(|e| e.to_string())?;
                    last_trust_key = Some((keys.0, Instant::now()));
                    trust_presses += 1;
                }
            } else {
                if trust_entered {
                    trust_finished = true;
                }
                if !staged
                    && recovering
                    && harness::input(kind, &screen).as_deref()
                        == Some(harness::rendered(pointer).as_str())
                {
                    staged = true;
                } else if !staged && resume_enter {
                    if harness::input(kind, &screen).is_some_and(|text| text.is_empty()) {
                        if let Some((state, detail, since)) = waits.observe("", Instant::now()) {
                            state_event(state, detail, since)?;
                        }
                        return Ok(());
                    }
                } else if !staged && harness::input_clear(kind, &screen) {
                    engine
                        .deliver(pane, pointer.to_owned(), false)
                        .await
                        .map_err(|e| e.to_string())?
                        .await
                        .map_err(|e| e.to_string())?;
                    staged = true;
                } else if staged {
                    match submission.observe(kind, &screen, Instant::now()) {
                        SubmissionAction::Wait => (),
                        SubmissionAction::Enter => {
                            before_enter()?;
                            engine
                                .input(pane, b"\r".to_vec())
                                .await
                                .map_err(|e| e.to_string())?;
                            submission.pressed(Instant::now());
                        }
                        SubmissionAction::Consumed => {
                            if let Some((state, detail, since)) = waits.observe("", Instant::now())
                            {
                                state_event(state, detail, since)?;
                            }
                            return Ok(());
                        }
                    }
                }
            }
        } else if !engine
            .list()
            .iter()
            .any(|session| session.panes.iter().any(|p| p.0 == pane))
        {
            return Err("owned seat exited while submitting its brief".into());
        }
        let state = if staged {
            "brief_not_taken"
        } else if seen_ready {
            "waiting_for_idle_input"
        } else {
            "seat_not_ready"
        };
        if let Some((state, detail, since)) = waits.observe(state, Instant::now()) {
            state_event(state, detail, since)?;
        }
        if let Some(state) = waits.stuck(Instant::now()) {
            return Err(super::state::stuck_reason(
                state,
                engine.screen(pane).as_ref(),
            ));
        }
        tokio::select! {
            received = updates.recv() => match received {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => (),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Err("seat stream closed".into()),
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => (),
        }
    }
}
