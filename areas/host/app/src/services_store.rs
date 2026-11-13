//! Each node's services as its catalog lists them (`GET /api/services`),
//! and CHROTE's rules for showing a change: a row says what was asked
//! ("starting…") until the node answers, and a failure stays in systemd's
//! words until the next action on that row.
//!
//! An action whose end is not known (the node said `unconfirmed`, or did
//! not answer in time) or that left its service moving is followed: that
//! one service is read again a few times, further apart each time, until it
//! settles. One that has not settled by then says so and is left to
//! Refresh. Nothing is read on a timer otherwise.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};

pub use parvi_host_contract::{LogLine, Service};
use parvi_host_contract::{ServiceList as List, ServiceLog, ServiceOne as One};

#[derive(Deserialize)]
struct Answer {
    service: Option<Service>,
    #[serde(default)]
    errors: Vec<String>,
    error: Option<parvi_core::Error>,
    #[serde(default)]
    outcome: String,
}

/// How long after an action, or the read before, each follow-up read waits;
/// the row is left to Refresh after the last.
pub const NO_ANSWER: &str = "The node did not answer";
pub const FOLLOW: [u64; 5] = [1_000, 2_000, 4_000, 8_000, 16_000];

/// What the app knows of how an action ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Ended {
    /// It did as told and the service has settled where the change aims.
    Done,
    /// It did as told, but the service settled elsewhere (a unit that
    /// failed as it started): the row shows that, and nothing claims the
    /// change, as in CHROTE.
    Elsewhere,
    /// systemd's words.
    Failed(String),
    /// Not known yet, or the service is still moving: read it again.
    Follow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Start,
    Stop,
    Restart,
}

impl Change {
    pub fn verb(self) -> &'static str {
        match self {
            Change::Start => "start",
            Change::Stop => "stop",
            Change::Restart => "restart",
        }
    }
    /// What the row says while the change runs.
    pub fn word(self) -> &'static str {
        match self {
            Change::Start => "starting…",
            Change::Stop => "stopping…",
            Change::Restart => "restarting…",
        }
    }
    pub fn past(self) -> &'static str {
        match self {
            Change::Start => "started",
            Change::Stop => "stopped",
            Change::Restart => "restarted",
        }
    }
    /// The state the change aims at.
    fn aim(self) -> &'static str {
        match self {
            Change::Stop => "stopped",
            Change::Start | Change::Restart => "running",
        }
    }
}

/// One node's catalog as last read.
#[derive(Default)]
pub struct NodeServices {
    pub list: Option<Vec<Service>>,
    /// Why the last read failed, in the node's words.
    pub error: Option<String>,
    pub reading: bool,
    /// When the last read landed, Unix ms.
    pub read_at: Option<u64>,
    /// Rows whose change runs, until a read after it lands.
    pub changing: HashMap<String, Change>,
    /// A row's last failure, until the next action on it.
    pub failures: HashMap<String, String>,
    /// Rows whose action's end is not known yet, and what the row says
    /// meanwhile; cleared when a read finds the service settled.
    pub unsettled: HashMap<String, String>,
    /// Each service's log as last read.
    pub logs: HashMap<String, Log>,
}

/// A service's latest log lines, or why they could not be read.
#[derive(Default)]
pub struct Log {
    pub lines: Vec<LogLine>,
    pub errors: Vec<String>,
    pub reading: bool,
}

impl Log {
    /// A log read landed.
    pub fn read(&mut self, status: u16, body: &[u8]) {
        self.reading = false;
        match serde_json::from_slice::<ServiceLog>(body) {
            Ok(l) if status == 200 => {
                self.lines = l.lines;
                self.errors = l.errors;
            }
            _ => self.errors = vec![words(status, body)],
        }
    }
}

#[derive(Default)]
pub struct Services {
    pub nodes: BTreeMap<String, NodeServices>,
    /// The row the keys act on: (node, service id).
    pub selected: Option<(String, String)>,
}

/// The word a state is shown with.
pub fn word(state: &str) -> &str {
    match state {
        "starting" => "starting…",
        "stopping" => "stopping…",
        s => s,
    }
}

/// A key that moves the chosen row, as CHROTE's list does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Down,
    Up,
    Home,
    End,
}

/// The list's groups, in its order: what needs the operator, what runs,
/// what is stopped (on purpose: a stop that failed is in Failed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Failed,
    Running,
    Stopped,
}

impl Group {
    pub fn of(state: &str) -> Group {
        match state {
            "running" | "starting" | "stopping" => Group::Running,
            "stopped" => Group::Stopped,
            _ => Group::Failed,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Group::Failed => "Failed",
            Group::Running => "Running",
            Group::Stopped => "Stopped",
        }
    }
}

impl Services {
    /// Moves the chosen row through `order` (every row, in the page's
    /// order). With none chosen, Down and Up choose the first.
    pub fn step(&mut self, order: &[(String, String)], step: Step) {
        if order.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|s| order.iter().position(|o| o == s));
        let last = order.len() - 1;
        let to = match (step, at) {
            (Step::Home, _) | (Step::Up | Step::Down, None) => 0,
            (Step::End, _) => last,
            (Step::Down, Some(i)) => (i + 1).min(last),
            (Step::Up, Some(i)) => i.saturating_sub(1),
        };
        self.selected = Some(order[to].clone());
    }

    /// The service on `node` with `id`, as last read.
    pub fn service(&self, node: &str, id: &str) -> Option<&Service> {
        self.nodes.get(node)?.row(id)
    }

    /// What Enter does to the chosen row: opens its address, if it has one.
    pub fn enter(&self) -> Option<String> {
        let (node, id) = self.selected.as_ref()?;
        let s = self.service(node, id)?;
        (!s.link.is_empty()).then(|| s.link.clone())
    }
}

/// The actions a service offers in its state, the one most wanted first:
/// a running one restarts or stops, a failed or stopped one starts, one
/// that is changing or not known offers none.
pub fn actions(state: &str) -> &'static [Change] {
    match state {
        "running" => &[Change::Restart, Change::Stop],
        "failed" | "partial" | "stopped" => &[Change::Start],
        _ => &[],
    }
}

/// How long ago, in a few characters: `<1m`, `12m`, `4h 12m`, `3d 4h`.
pub fn ago(since: u64, now: u64) -> String {
    let secs = now.saturating_sub(since) / 1000;
    if secs < 60 {
        "<1m".into()
    } else {
        crate::host_store::uptime(secs)
    }
}

/// What Send hands over for a service: which, in what state since when,
/// its units' states and its address.
pub fn reference(s: &Service, now: u64) -> String {
    let mut out = format!("service {} ({}): {}", s.id, s.name, word(&s.state));
    if let Some(since) = s.since() {
        out.push_str(&format!(" for {}", ago(since, now)));
    }
    if s.restarts() > 0 {
        out.push_str(&format!(", restarted {} times by systemd", s.restarts()));
    }
    let units: Vec<String> = s
        .components
        .iter()
        .map(|c| format!("{} {}", c.unit, c.state))
        .collect();
    if !units.is_empty() {
        out.push_str(&format!("; {}", units.join(", ")));
    }
    if !s.link.is_empty() {
        out.push_str(&format!("; {}", s.link));
    }
    out
}

impl NodeServices {
    /// A list read landed: the body, or the refusal's words.
    pub fn listed(&mut self, status: u16, body: &[u8], now: u64) {
        self.reading = false;
        match serde_json::from_slice::<List>(body) {
            Ok(l) if status == 200 => {
                self.unsettled.retain(|id, _| {
                    l.services
                        .iter()
                        .any(|s| s.id == *id && (s.state == "starting" || s.state == "stopping"))
                });
                self.list = Some(l.services);
                self.error = None;
                self.read_at = Some(now);
            }
            _ => self.error = Some(words(status, body)),
        }
    }

    /// The state shown for a row: the change asked for while it runs.
    pub fn shown<'a>(&'a self, s: &'a Service) -> &'a str {
        match self.changing.get(&s.id) {
            Some(c) => c.word(),
            None => word(&s.state),
        }
    }

    /// An action answered: the row takes the node's read of it. A node
    /// that did not answer in time may still be acting.
    pub fn answered(&mut self, id: &str, change: Change, status: u16, body: &[u8]) -> Ended {
        self.changing.remove(id);
        let silent = status == 0 && String::from_utf8_lossy(body).starts_with(NO_ANSWER);
        let answer: Option<Answer> = serde_json::from_slice(body).ok();
        if let Some(s) = answer.as_ref().and_then(|a| a.service.clone()) {
            self.put(s);
        }
        let ended = match answer {
            _ if silent => {
                self.unsettled.insert(
                    id.into(),
                    format!(
                        "The node did not answer the {}; reading it again.",
                        change.verb()
                    ),
                );
                Ended::Follow
            }
            Some(a) if a.outcome == "unconfirmed" => {
                self.unsettled.insert(id.into(), a.errors.join("; "));
                Ended::Follow
            }
            Some(a) if !a.errors.is_empty() => Ended::Failed(a.errors.join("; ")),
            Some(Answer { error: Some(e), .. }) => Ended::Failed(e.message),
            Some(_) if status < 300 => match self.row(id) {
                _ if self.moving(id) => Ended::Follow,
                Some(s) if s.state == change.aim() => Ended::Done,
                _ => Ended::Elsewhere,
            },
            _ => Ended::Failed(words(status, body)),
        };
        match &ended {
            Ended::Failed(f) => {
                self.failures.insert(id.to_string(), f.clone());
            }
            _ => {
                self.failures.remove(id);
            }
        }
        ended
    }

    /// A follow-up read of one service landed. Returns whether to read it
    /// again: it is still moving, or the read failed, and `last` is not set.
    /// After the last read the row says what it last knew, as of `at`.
    pub fn followed(&mut self, id: &str, status: u16, body: &[u8], last: bool, at: &str) -> bool {
        let read = serde_json::from_slice::<One>(body)
            .ok()
            .filter(|_| status == 200);
        let got = read.is_some();
        if let Some(one) = read {
            self.put(one.service);
        }
        if got && !self.moving(id) {
            self.unsettled.remove(id);
            return false;
        }
        if !last {
            return true;
        }
        let note = match self.row(id) {
            Some(s) if got => format!("Still {} at {at}; Refresh reads it again.", s.state),
            _ => format!(
                "Not read since the action ({}); Refresh reads it again.",
                words(status, body)
            ),
        };
        self.unsettled.insert(id.into(), note);
        false
    }

    pub fn row(&self, id: &str) -> Option<&Service> {
        self.list.iter().flatten().find(|r| r.id == id)
    }

    fn moving(&self, id: &str) -> bool {
        self.row(id)
            .is_some_and(|s| s.state == "starting" || s.state == "stopping")
    }

    fn put(&mut self, s: Service) {
        if let Some(list) = &mut self.list
            && let Some(row) = list.iter_mut().find(|r| r.id == s.id)
        {
            *row = s;
        }
    }
}

/// A refusal's words: `{error}` if it has one, else its text.
pub fn words(status: u16, body: &[u8]) -> String {
    #[derive(Deserialize)]
    struct E {
        error: parvi_core::Error,
    }
    if let Ok(e) = serde_json::from_slice::<E>(body) {
        return e.error.message;
    }
    let text = String::from_utf8_lossy(body).trim().to_string();
    if text.is_empty() {
        format!("The node answered {status}.")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> NodeServices {
        let mut n = NodeServices::default();
        n.listed(
            200,
            br#"{"services":[{"id":"example","name":"Example","description":"d","link":"http://h:8080/x","state":"stopped","controllable":true,"components":[{"unit":"example.service","state":"inactive","sub":"dead","since":1000}]},
                {"id":"room","name":"Room","state":"running","controllable":false,"components":[{"unit":"room.service","state":"active"}]}]}"#,
            1,
        );
        n
    }

    #[test]
    fn keys_move_the_chosen_row_and_enter_opens_its_address() {
        let mut s = Services::default();
        s.nodes.insert("a".into(), node());
        let row = |id: &str| ("a".to_string(), id.to_string());
        let order = [row("example"), row("room")];
        s.step(&order, Step::Up);
        assert_eq!(s.selected, Some(row("example")), "none chosen: the first");
        assert_eq!(s.enter().as_deref(), Some("http://h:8080/x"));
        s.step(&order, Step::Down);
        s.step(&order, Step::Down);
        assert_eq!(s.selected, Some(row("room")), "stays on the last");
        assert_eq!(s.enter(), None, "no address");
        s.step(&order, Step::Home);
        assert_eq!(s.selected, Some(row("example")));
        s.step(&order, Step::End);
        assert_eq!(s.selected, Some(row("room")));
        s.step(&[], Step::Home);
        assert_eq!(s.selected, Some(row("room")), "an empty page moves nothing");
    }

    #[test]
    fn a_row_says_starting_until_the_node_answers() {
        let mut n = node();
        n.changing.insert("example".into(), Change::Start);
        let s = n.list.as_ref().unwrap()[0].clone();
        assert_eq!(n.shown(&s), "starting…");
        let ended = n.answered(
            "example",
            Change::Start,
            200,
            br#"{"service":{"id":"example","name":"Example","state":"running","controllable":true},"errors":[],"outcome":"done"}"#,
        );
        assert_eq!(ended, Ended::Done);
        assert!(n.changing.is_empty());
        assert_eq!(n.list.as_ref().unwrap()[0].state, "running");
    }

    #[test]
    fn a_failure_stays_in_systemds_words_until_the_next_action() {
        let mut n = node();
        let ended = n.answered(
            "example",
            Change::Start,
            502,
            br#"{"service":{"id":"example","name":"Example","state":"failed","controllable":true},"errors":["Job failed","See journal"],"outcome":"failed"}"#,
        );
        assert_eq!(ended, Ended::Failed("Job failed; See journal".into()));
        n.listed(
            200,
            br#"{"services":[{"id":"example","name":"Example","state":"stopped","controllable":true}]}"#,
            2,
        );
        assert_eq!(
            n.failures["example"], "Job failed; See journal",
            "a read keeps it"
        );
        n.answered(
            "example",
            Change::Start,
            200,
            br#"{"service":null,"errors":[]}"#,
        );
        assert!(!n.failures.contains_key("example"));
    }

    #[test]
    fn an_unknown_end_is_followed_until_it_settles_or_is_left_to_refresh() {
        let mut n = node();
        let silent = format!("{} 50 s", NO_ANSWER);
        assert_eq!(
            n.answered("example", Change::Restart, 0, silent.as_bytes()),
            Ended::Follow,
            "the node may still be restarting it"
        );
        assert!(n.unsettled.contains_key("example"));
        assert!(
            !n.failures.contains_key("example"),
            "not a failure: not known"
        );
        let moving = br#"{"service":{"id":"example","name":"Example","state":"starting","controllable":true}}"#;
        assert!(n.followed("example", 200, moving, false, "01:00"));
        assert!(
            !n.followed("example", 200, moving, true, "01:00"),
            "the last read stops"
        );
        assert!(n.unsettled["example"].starts_with("Still starting at"));
        let settled = br#"{"service":{"id":"example","name":"Example","state":"running","controllable":true}}"#;
        assert!(!n.followed("example", 200, settled, false, "01:01"));
        assert!(!n.unsettled.contains_key("example"));

        let unconfirmed = br#"{"service":{"id":"example","name":"Example","state":"stopping","controllable":true},"errors":["systemctl stop gave no answer in 15 s; systemd may still finish it"],"outcome":"unconfirmed"}"#;
        assert_eq!(
            n.answered("example", Change::Stop, 202, unconfirmed),
            Ended::Follow
        );
        assert!(n.unsettled["example"].contains("may still finish"));
        n.listed(
            200,
            br#"{"services":[{"id":"example","name":"Example","state":"stopped","controllable":true}]}"#,
            3,
        );
        assert!(
            n.unsettled.is_empty(),
            "a Refresh that finds it settled clears it"
        );
    }

    #[test]
    fn does_not_claim_a_start_when_the_node_reports_it_failed() {
        let mut n = node();
        let ended = n.answered(
            "example",
            Change::Start,
            200,
            br#"{"service":{"id":"example","name":"Example","state":"failed","controllable":true},"errors":[],"outcome":"done"}"#,
        );
        assert_eq!(ended, Ended::Elsewhere);
        assert!(!n.failures.contains_key("example"), "its state says it");
        assert_eq!(n.list.as_ref().unwrap()[0].state, "failed");
        let stopped = br#"{"service":{"id":"example","name":"Example","state":"stopped","controllable":true},"errors":[],"outcome":"done"}"#;
        assert_eq!(
            n.answered("example", Change::Stop, 200, stopped),
            Ended::Done
        );
    }

    #[test]
    fn a_plain_refusal_is_shown_as_its_text() {
        let mut n = NodeServices::default();
        n.listed(503, b"Service controller is unavailable", 1);
        assert_eq!(
            n.error.as_deref(),
            Some("Service controller is unavailable")
        );
        n.listed(
            503,
            br#"{"error":{"code":"unavailable","message":"No service catalog"}}"#,
            1,
        );
        assert_eq!(n.error.as_deref(), Some("No service catalog"));
    }

    #[test]
    fn a_coded_action_refusal_shows_its_message() {
        let mut n = node();
        let ended = n.answered(
            "example",
            Change::Restart,
            400,
            br#"{"error":{"code":"bad_request","message":"This service is read-only"}}"#,
        );
        assert_eq!(ended, Ended::Failed("This service is read-only".into()));
        assert_eq!(n.failures["example"], "This service is read-only");
    }

    #[test]
    fn each_state_offers_its_own_actions_and_its_group() {
        assert_eq!(actions("running"), [Change::Restart, Change::Stop]);
        assert_eq!(actions("stopped"), [Change::Start], "not Start and Restart");
        assert_eq!(actions("failed"), [Change::Start]);
        assert_eq!(actions("starting"), []);
        assert_eq!(actions("unknown"), []);
        assert_eq!(Group::of("stopped"), Group::Stopped, "stopped on purpose");
        assert_eq!(Group::of("failed"), Group::Failed);
        assert_eq!(Group::of("partial"), Group::Failed);
        assert_eq!(Group::of("unknown"), Group::Failed);
        assert_eq!(Group::of("stopping"), Group::Running);
    }

    #[test]
    fn a_service_is_handed_over_whole() {
        let n = node();
        assert_eq!(
            reference(&n.list.as_ref().unwrap()[0], 1000 + 12 * 60_000),
            "service example (Example): stopped for 12m; example.service inactive; http://h:8080/x"
        );
        assert_eq!(
            reference(&n.list.as_ref().unwrap()[1], 0),
            "service room (Room): running; room.service active"
        );
    }

    #[test]
    fn a_log_is_its_lines_or_why_not() {
        let mut log = Log {
            reading: true,
            ..Log::default()
        };
        log.read(
            200,
            br#"{"lines":[{"at":1,"text":"listening","error":false}],"errors":[]}"#,
        );
        assert!(!log.reading);
        assert_eq!(log.lines[0].text, "listening");
        log.read(
            404,
            br#"{"error":{"code":"not_found","message":"No such service"}}"#,
        );
        assert_eq!(log.errors, ["No such service"]);
    }
}
