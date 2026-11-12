//! Agent events, as CHROTE has them: a harness's own hooks report that its
//! agent finished a turn or needs the operator, and the node keeps the last
//! report per session until someone looks at it (docs/protocol.md, Agent
//! events). Nothing here guesses from the screen; that is `waiting`.
//!
//! A launch asked to notify installs the hooks ([`hooks`]); each runs
//! `parvi hook`, which posts to [`routes`] with the session's hub id and
//! the event in the address. The node keeps the report in memory: a restart forgets,
//! which is what a notice should do. A report is dropped with its session.

pub mod hooks;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::post;
use parvi_core::Error;
use parvi_node_kit::Failure;
use serde::Deserialize;
use serde_json::Value;

use crate::hub::Hub;
use parvi_term::engine::SessionInfo;

/// The most a summary says: a hint for a toast, not a transcript.
const SUMMARY_CHARS: usize = 200;

/// The most of a hook's payload that is read.
pub use parvi_node_kit::hook::MAX_PAYLOAD;

/// The path hooks post to.
pub const PATH: &str = "/api/agent/event";

/// What a hook reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The agent finished a turn.
    Finished,
    /// The agent waits on the operator: a permission, or idle input.
    NeedsInput,
}

impl Kind {
    pub fn parse(word: &str) -> Option<Kind> {
        match word {
            "finished" => Some(Kind::Finished),
            "needs-input" => Some(Kind::NeedsInput),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::Finished => "finished",
            Kind::NeedsInput => "needs-input",
        }
    }
}

/// A session's last report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: Kind,
    /// When the node heard it, in milliseconds since the Unix epoch.
    pub at: u64,
    /// What the agent said, cut short; may be empty.
    pub summary: String,
    /// Someone has looked at the session since.
    pub seen: bool,
}

/// The last report of every session that has made one.
#[derive(Default)]
pub struct Events {
    by_session: Mutex<HashMap<String, Event>>,
}

impl Events {
    /// Replaces the session's last report. A new one is unseen, whatever
    /// became of the one before: it is news again. Its `at` is later than
    /// the one before's, even within a millisecond, so it names the report.
    pub fn record(&self, session: &str, kind: Kind, summary: String) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let mut map = self.by_session.lock().unwrap();
        let at = map.get(session).map_or(now, |e| now.max(e.at + 1));
        map.insert(
            session.to_owned(),
            Event {
                kind,
                at,
                summary,
                seen: false,
            },
        );
    }

    /// Marks the session's report heard at `at` seen; a newer one stays
    /// news. True if that changed anything.
    pub fn seen(&self, session: &str, at: u64) -> bool {
        let mut map = self.by_session.lock().unwrap();
        match map.get_mut(session) {
            Some(event) if !event.seen && event.at == at => {
                event.seen = true;
                true
            }
            _ => false,
        }
    }

    /// The reports of these sessions, forgetting those of sessions gone: the
    /// list is where the live set is known, so this is where the store is
    /// kept to it.
    pub fn of(&self, sessions: &[SessionInfo]) -> HashMap<String, Event> {
        let mut map = self.by_session.lock().unwrap();
        map.retain(|id, _| sessions.iter().any(|s| &s.id == id));
        map.clone()
    }
}

/// The one message a hook's payload carries, as plain one-line text, cut to
/// [`SUMMARY_CHARS`]. Claude Code's Notification says `message`; its Stop and
/// Codex's notify carry the last assistant message.
pub fn summary(payload: &[u8]) -> String {
    let Ok(doc) = serde_json::from_slice::<Value>(payload) else {
        return String::new();
    };
    let text = [
        "message",
        "last-assistant-message",
        "last_assistant_message",
    ]
    .iter()
    .find_map(|key| doc.get(key)?.as_str())
    .unwrap_or_default();
    let mut out = String::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().filter(|c| !c.is_control()));
    }
    match out.char_indices().nth(SUMMARY_CHARS) {
        Some((cut, _)) => format!("{}…", &out[..cut]),
        None => out,
    }
}

/// Whether a payload reports Codex's side task that names the thread, not
/// the operator's turn: its last message is only `{"title": …}`. Codex
/// notifies for it just after the real turn, which it would otherwise
/// replace, with a second report the operator did not ask for.
fn names_the_thread(payload: &[u8]) -> bool {
    serde_json::from_slice::<Value>(payload)
        .ok()
        .and_then(|doc| {
            let text = doc.get("last-assistant-message")?.as_str()?.to_owned();
            let inner = serde_json::from_str::<Value>(&text).ok()?;
            let fields = inner.as_object()?;
            Some(fields.len() == 1 && fields.contains_key("title"))
        })
        .unwrap_or(false)
}

#[derive(Deserialize)]
struct Report {
    session: String,
    event: String,
}

/// `POST /api/agent/event?session=<hub id>&event=finished|needs-input`, the
/// hook's payload as the body.
pub fn routes(hub: Arc<Hub>) -> Router {
    Router::new().route(PATH, post(report)).with_state(hub)
}

async fn report(
    State(hub): State<Arc<Hub>>,
    Query(report): Query<Report>,
    body: Bytes,
) -> Result<StatusCode, Failure> {
    let Some(kind) = Kind::parse(&report.event) else {
        return Err(Failure(
            StatusCode::BAD_REQUEST,
            Error::new("bad_event", "event is finished or needs-input"),
        ));
    };
    let body = &body[..body.len().min(MAX_PAYLOAD)];
    if names_the_thread(body) {
        return Ok(StatusCode::NO_CONTENT);
    }
    match hub.record_event(&report.session, kind, summary(body)) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(error) => Err(Failure(
            StatusCode::NOT_FOUND,
            Error::new("unknown_session", format!("{error:#}")),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codexs_thread_title_is_no_report() {
        // As Codex 0.167 notifies just after a turn that answered "ok".
        let title = br#"{"type":"agent-turn-complete","client":"codex-tui","last-assistant-message":"{\"title\":\"Reply with ok\"}"}"#;
        assert!(names_the_thread(title));
        let turn = br#"{"type":"agent-turn-complete","last-assistant-message":"ok"}"#;
        assert!(!names_the_thread(turn));
        let json_answer = br#"{"last-assistant-message":"{\"title\":\"x\",\"body\":\"y\"}"}"#;
        assert!(!names_the_thread(json_answer));
        assert!(!names_the_thread(
            br#"{"message":"Claude needs your permission"}"#
        ));
    }

    #[test]
    fn a_summary_is_the_payloads_message_on_one_line_cut_short() {
        assert_eq!(
            summary(br#"{"message":"Claude needs\nyour  permission"}"#),
            "Claude needs your permission"
        );
        assert_eq!(
            summary(br#"{"type":"agent-turn-complete","last-assistant-message":"Done."}"#),
            "Done."
        );
        assert_eq!(summary(b"not json"), "");
        assert_eq!(summary(br#"{"session_id":"x"}"#), "");
        let long = format!(r#"{{"message":"{}"}}"#, "é".repeat(300));
        let cut = summary(long.as_bytes());
        assert_eq!(cut.chars().count(), SUMMARY_CHARS + 1);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn a_report_is_news_until_seen_and_goes_with_its_session() {
        let events = Events::default();
        let live = |ids: &[&str]| -> Vec<SessionInfo> {
            ids.iter()
                .map(|id| SessionInfo {
                    id: (*id).into(),
                    name: String::new(),
                    panes: vec![],
                    seat: None,
                    facts: None,
                    activity: Default::default(),
                    waiting: vec![],
                    user: None,
                })
                .collect()
        };
        events.record("$1", Kind::Finished, "Done.".into());
        let first = events.of(&live(&["$1"]))["$1"].at;
        assert!(events.seen("$1", first));
        assert!(!events.seen("$1", first), "seen once");
        events.record("$1", Kind::NeedsInput, String::new());
        let second = events.of(&live(&["$1"]))["$1"].at;
        assert!(
            second > first,
            "a report within the millisecond is still later"
        );
        assert!(
            !events.seen("$1", first),
            "seeing the old report leaves the new one news"
        );
        let now = events.of(&live(&["$1", "$2"]));
        assert_eq!(now["$1"].kind, Kind::NeedsInput);
        assert!(!now["$1"].seen, "a new report is news again");
        assert!(events.of(&live(&["$2"])).is_empty());
        assert!(
            events.of(&live(&["$1"])).is_empty(),
            "forgotten, not hidden"
        );
    }
}
