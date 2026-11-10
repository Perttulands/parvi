//! Report long seat waits without putting a timeout on a working agent.
use std::time::{Duration, Instant};

pub(super) struct WaitTracker {
    state: &'static str,
    since: Instant,
    timestamp: String,
    recorded: &'static str,
}
impl WaitTracker {
    pub fn new(now: Instant) -> Self {
        Self {
            state: "",
            since: now,
            timestamp: String::new(),
            recorded: "",
        }
    }
    pub fn observe(
        &mut self,
        state: &'static str,
        now: Instant,
    ) -> Option<(&'static str, &'static str, String)> {
        if self.state != state {
            self.state = state;
            self.since = now;
            self.timestamp = crate::run_storage::now();
        }
        if state.is_empty() {
            if !self.recorded.is_empty() {
                self.recorded = "";
                return Some(("working", "", self.timestamp.clone()));
            }
        } else if now.saturating_duration_since(self.since) >= Duration::from_secs(60)
            && self.recorded != state
        {
            self.recorded = state;
            return Some((state, detail(state), self.timestamp.clone()));
        }
        None
    }
}
fn detail(state: &str) -> &'static str {
    match state {
        "seat_not_ready" => {
            "the seat has not reached its ready prompt; open the seat to see its screen"
        }
        "waiting_for_idle_input" => {
            "the brief waits to be pasted: the agent is busy or the input line holds unsent text"
        }
        "brief_not_taken" => {
            "the brief is pasted but not sent: Enter did not send it, or the input line also holds text the operator does not send"
        }
        "turn_ended_without_sentinel" => {
            "the agent ended a turn without the completion sentinel; ask it to finish or send the sentinel"
        }
        "background_work_pending" => {
            "the agent ended its turn with background work that has not resumed"
        }
        _ => "",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_wait_is_reported_once_and_closed_when_work_resumes() {
        let start = Instant::now();
        let mut wait = WaitTracker::new(start);
        assert!(wait.observe("brief_not_taken", start).is_none());
        assert!(
            wait.observe("brief_not_taken", start + Duration::from_secs(59))
                .is_none()
        );
        assert_eq!(
            wait.observe("brief_not_taken", start + Duration::from_secs(60))
                .unwrap()
                .0,
            "brief_not_taken"
        );
        assert!(
            wait.observe("brief_not_taken", start + Duration::from_secs(70))
                .is_none()
        );
        assert_eq!(
            wait.observe("", start + Duration::from_secs(71)).unwrap().0,
            "working"
        );
        assert!(wait.observe("", start + Duration::from_secs(72)).is_none());
    }
}
