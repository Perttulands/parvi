//! Report long seat waits, remind a seat that stops without its done line,
//! and stop a step whose seat cannot go on without someone.
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
    /// The state that has lasted past its limit: a seat that never got its
    /// brief going cannot progress without someone.
    pub fn stuck(&self, now: Instant) -> Option<&'static str> {
        let limit = match self.state {
            "brief_not_taken" => Duration::from_secs(90),
            "seat_not_ready" | "waiting_for_idle_input" => Duration::from_secs(180),
            _ => return None,
        };
        (now.saturating_duration_since(self.since) >= limit).then_some(self.state)
    }
}

/// Why a seat that never took its brief stopped the step, in plain words
/// with what its screen says.
pub(super) fn stuck_reason(state: &str, screen: Option<&parvi_term::Screen>) -> String {
    let why = match state {
        "brief_not_taken" => "the agent did not take its brief",
        "seat_not_ready" => "the agent never reached its prompt",
        _ => "the agent stayed busy and never took its brief",
    };
    let said = screen.map(screen_words).unwrap_or_default();
    if said.is_empty() {
        format!("seat_stuck: {why}. Open its terminal to see why, then resume the run.")
    } else {
        format!("seat_stuck: {why}. Its screen says: {said}")
    }
}

/// The lines of a seat's screen that say what went wrong: the last one that
/// reads as an error, or else its last few lines.
pub(super) fn screen_words(screen: &parvi_term::Screen) -> String {
    let lines: Vec<String> = screen
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .filter(|cell| cell.width != 0)
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .filter(|line| !line.is_empty() && line.chars().any(|c| c.is_alphanumeric()))
        .collect();
    let error = lines.iter().rev().find(|line| {
        let lower = line.to_lowercase();
        [
            "error",
            "unavailable",
            "not available",
            "no longer",
            "not supported",
            "not found",
            "failed",
        ]
        .iter()
        .any(|word| lower.contains(word))
    });
    let words = match error {
        Some(line) => line.clone(),
        None => lines[lines.len().saturating_sub(3)..].join(" / "),
    };
    let words = words
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .to_owned();
    match words.char_indices().nth(300) {
        Some((at, _)) => format!("{}…", &words[..at]),
        None => words,
    }
}
/// How long a seat may sit at an empty prompt, after its harness recorded the
/// end of its turn, before Parvi reminds it to finish.
pub(super) const ENDED_GRACE: Duration = Duration::from_secs(20);
/// The same when only the screen shows an empty prompt: the harness's own
/// record has not said the turn ended.
pub(super) const QUIET_GRACE: Duration = Duration::from_secs(120);
/// Reminders before the step stops and asks for the operator.
pub(super) const REMINDERS: usize = 2;

/// What to do about a seat that stopped without its done line.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Nudge {
    Remind,
    GiveUp,
}

/// Watches a dispatched turn for an agent that stopped working without
/// saying it was done. Any new output, or a seat that is not idle, starts the
/// wait again; the grace is short when the harness recorded the turn's end.
pub(super) struct Reminders {
    quiet_since: Option<Instant>,
    pub sent: usize,
}
impl Reminders {
    pub fn new(sent: usize) -> Self {
        Self {
            quiet_since: None,
            sent,
        }
    }
    pub fn observe(
        &mut self,
        idle: bool,
        ended: bool,
        activity: bool,
        now: Instant,
    ) -> Option<Nudge> {
        if !idle || activity {
            self.quiet_since = None;
            return None;
        }
        let since = *self.quiet_since.get_or_insert(now);
        let grace = if ended { ENDED_GRACE } else { QUIET_GRACE };
        if now.saturating_duration_since(since) < grace {
            return None;
        }
        self.quiet_since = None;
        if self.sent >= REMINDERS {
            return Some(Nudge::GiveUp);
        }
        self.sent += 1;
        Some(Nudge::Remind)
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
    fn a_seat_that_stops_without_its_done_line_is_reminded_twice_then_given_up() {
        let start = Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        let mut nudge = Reminders::new(0);
        // Its harness recorded the end of the turn: the grace is short.
        assert_eq!(nudge.observe(true, true, false, at(0)), None);
        assert_eq!(nudge.observe(true, true, false, at(19)), None);
        assert_eq!(
            nudge.observe(true, true, false, at(20)),
            Some(Nudge::Remind)
        );
        // It answers the reminder: output starts the wait again.
        assert_eq!(nudge.observe(true, true, false, at(25)), None);
        assert_eq!(nudge.observe(false, false, true, at(30)), None);
        assert_eq!(nudge.observe(true, true, false, at(31)), None);
        assert_eq!(nudge.observe(true, true, false, at(50)), None);
        assert_eq!(
            nudge.observe(true, true, false, at(51)),
            Some(Nudge::Remind)
        );
        assert_eq!(nudge.observe(true, true, false, at(52)), None);
        assert_eq!(
            nudge.observe(true, true, false, at(72)),
            Some(Nudge::GiveUp)
        );
    }
    #[test]
    fn a_seat_that_never_takes_its_brief_says_why_in_its_own_words() {
        let start = Instant::now();
        let mut wait = WaitTracker::new(start);
        wait.observe("brief_not_taken", start);
        assert_eq!(wait.stuck(start + Duration::from_secs(89)), None);
        assert_eq!(
            wait.stuck(start + Duration::from_secs(90)),
            Some("brief_not_taken")
        );
        wait.observe("", start + Duration::from_secs(91));
        assert_eq!(wait.stuck(start + Duration::from_secs(500)), None);

        let row = |text: &str| {
            text.chars()
                .map(|c| parvi_term::Cell {
                    text: c.to_string(),
                    width: 1,
                    foreground: parvi_term::Color::Default,
                    background: parvi_term::Color::Default,
                    bold: false,
                    dim: false,
                    italic: false,
                    inverse: false,
                })
                .collect()
        };
        let screen = parvi_term::Screen {
            rows: [
                "OpenAI Codex",
                "",
                "■ The model gpt-5.4-mini is no longer available",
                "",
                "› ",
            ]
            .map(row)
            .to_vec(),
            cursor: parvi_term::Cursor {
                row: 4,
                column: 2,
                visible: true,
            },
        };
        assert_eq!(
            stuck_reason("brief_not_taken", Some(&screen)),
            "seat_stuck: the agent did not take its brief. Its screen says: The model gpt-5.4-mini is no longer available"
        );
    }
    #[test]
    fn a_quiet_screen_alone_waits_longer_and_a_working_seat_is_never_nudged() {
        let start = Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        let mut nudge = Reminders::new(0);
        assert_eq!(nudge.observe(true, false, false, at(0)), None);
        assert_eq!(nudge.observe(true, false, false, at(119)), None);
        assert_eq!(
            nudge.observe(true, false, false, at(120)),
            Some(Nudge::Remind)
        );
        let mut working = Reminders::new(0);
        for s in 0..1000 {
            assert_eq!(working.observe(s % 2 == 0, false, s % 2 == 1, at(s)), None);
        }
        // After a restart the reminders already sent still count.
        let mut resumed = Reminders::new(REMINDERS);
        assert_eq!(resumed.observe(true, true, false, at(0)), None);
        assert_eq!(
            resumed.observe(true, true, false, at(20)),
            Some(Nudge::GiveUp)
        );
    }
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
