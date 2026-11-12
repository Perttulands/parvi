use crate::{Coordinator, Error};
use parvi_mission_model::RunProjection;
use std::time::Duration;

/// The condition a driver is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitUntil {
    NeedsYou,
    Final,
    AnyChange,
}

/// A projection at the point a wait answered.
pub struct RunWait {
    pub outcome: &'static str,
    pub cursor: u64,
    pub run: RunProjection,
}

impl Coordinator {
    /// Subscribe before reading the ledger so an append cannot fall between the
    /// projection and the wait. Each caller has its own broadcast receiver.
    pub async fn wait_run(
        &self,
        id: &str,
        until: WaitUntil,
        since: Option<u64>,
        timeout: Duration,
    ) -> Result<RunWait, Error> {
        let mut changes = self.subscribe();
        let deadline = tokio::time::Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| Error::new("bad_request", "timeout is too large"))?;
        loop {
            let run = self.run(id)?;
            let cursor = run.event_count as u64;
            let since = since.unwrap_or(0);
            if since > cursor {
                return Err(Error::new(
                    "bad_request",
                    "since is past the run's last event",
                ));
            }
            let needs_you = run
                .waiting_gates
                .iter()
                .any(|gate| gate.requested_seq > since)
                || (run.status == "blocked"
                    && run
                        .events
                        .iter()
                        .rev()
                        .find(|event| event.kind == "run_blocked")
                        .is_some_and(|event| event.seq > since));
            let outcome = if run.r#final {
                Some("final")
            } else if needs_you && (until != WaitUntil::Final || run.status == "blocked") {
                // A blocked run goes nowhere until someone acts, so even a
                // wait for the end answers it.
                Some("needs-you")
            } else if until == WaitUntil::AnyChange && cursor > since {
                Some("changed")
            } else if tokio::time::Instant::now() >= deadline {
                Some("timeout")
            } else {
                None
            };
            if let Some(outcome) = outcome {
                return Ok(RunWait {
                    outcome,
                    cursor,
                    run,
                });
            }
            // Lagged receivers re-read the ledger; unrelated runs are harmless.
            let _ = tokio::time::timeout_at(deadline, changes.recv()).await;
        }
    }
}
