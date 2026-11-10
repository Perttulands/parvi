//! The terminals frames whose contents come from the engine and the
//! launcher: the sessions list, the launcher, search results, and what
//! became of a send. The rest are built in the contract.

use std::collections::HashMap;

use parvi_core::json::push_string as push_json_string;
use parvi_term::engine::SessionInfo;
use parvi_terminals_contract::{LAUNCHER, MATCH, Outcome, SEARCHED, SESSIONS};

use crate::agent_events::Event;
use crate::launch::Config;

/// `0x01` and a JSON array: `[{"id":"$0","name":"a","panes":[{"id":1,"cols":80,"rows":24}]}]`.
/// A session whose sizing seat a client holds also has `"seat"`: `"mine"`
/// for the client `me`, `"other"` for anyone else. A pane whose screen asks
/// the operator something has `"waiting"` with the question. A session on
/// another Unix user's tmux server has `"user"`. A session whose agent
/// reported has `"event"`, its last report (`crate::agent_events`).
pub fn sessions(sessions: &[SessionInfo], events: &HashMap<String, Event>, me: u64) -> Vec<u8> {
    let mut json = String::from("[");
    for (i, session) in sessions.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str("{\"id\":");
        push_json_string(&mut json, &session.id);
        json.push_str(",\"name\":");
        push_json_string(&mut json, &session.name);
        if let Some(user) = &session.user {
            json.push_str(",\"user\":");
            push_json_string(&mut json, user);
        }
        json.push_str(",\"panes\":[");
        for (j, (id, cols, rows)) in session.panes.iter().enumerate() {
            if j > 0 {
                json.push(',');
            }
            json.push_str(&format!("{{\"id\":{id},\"cols\":{cols},\"rows\":{rows}"));
            if let Some((_, question)) = session.waiting.iter().find(|(pane, _)| pane == id) {
                json.push_str(",\"waiting\":");
                push_json_string(&mut json, question);
            }
            json.push('}');
        }
        json.push(']');
        match session.seat {
            Some(holder) if holder == me => json.push_str(",\"seat\":\"mine\""),
            Some(_) => json.push_str(",\"seat\":\"other\""),
            None => {}
        }
        if let Some(facts) = &session.facts {
            json.push_str(&format!(
                ",\"windows\":{},\"foreign\":{},\"pinned\":{},\"mouse\":{}",
                facts.windows, facts.foreign_clients, facts.pinned, facts.mouse
            ));
            if !facts.command.is_empty() {
                json.push_str(",\"command\":");
                push_json_string(&mut json, &facts.command);
            }
            if !facts.path.is_empty() {
                json.push_str(",\"cwd\":");
                push_json_string(&mut json, &facts.path);
            }
        }
        json.push_str(&format!(",\"active\":{}", session.activity.active));
        if let Some(ms) = session
            .activity
            .last_output
            .and_then(|at| at.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        {
            json.push_str(&format!(",\"last\":{}", ms.as_millis()));
        }
        if let Some(event) = events.get(&session.id) {
            json.push_str(&format!(
                ",\"event\":{{\"kind\":\"{}\",\"at\":{},\"seen\":{}",
                event.kind.word(),
                event.at,
                event.seen
            ));
            if !event.summary.is_empty() {
                json.push_str(",\"summary\":");
                push_json_string(&mut json, &event.summary);
            }
            json.push('}');
        }
        json.push('}');
    }
    json.push(']');
    let mut frame = vec![SESSIONS];
    frame.extend_from_slice(json.as_bytes());
    frame
}

/// `0x02` and the launcher's configuration as JSON, see [`Config::json`],
/// with `"users"`: the other Unix users a session can start for, if any, and
/// `"notifyOff"`: why a launch asked to notify gets no completion hooks
/// here, if it doesn't.
pub fn launcher(config: &Config, users: &[String], notify_off: Option<&str>) -> Vec<u8> {
    let mut json = config.json();
    if !users.is_empty() {
        json.pop();
        json.push_str(",\"users\":[");
        for (i, user) in users.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            push_json_string(&mut json, user);
        }
        json.push_str("]}");
    }
    if let Some(why) = notify_off {
        json.pop();
        json.push_str(",\"notifyOff\":");
        push_json_string(&mut json, why);
        json.push('}');
    }
    [&[LAUNCHER][..], json.as_bytes()].concat()
}

/// `0x26`, search id u32, pane u32, line u32, then UTF-8 `before\nline\nafter`.
/// `line` counts from the pane's oldest history line.
pub fn found(id: u32, found: &parvi_term::search::Match) -> Vec<u8> {
    let text = found.context.join("\n");
    [
        &[MATCH][..],
        &id.to_le_bytes(),
        &found.pane.to_le_bytes(),
        &found.line.to_le_bytes(),
        text.as_bytes(),
    ]
    .concat()
}

/// `0x27`, search id u32, matches u32, panes u32, truncated u8: the search
/// has ended, having read this many panes.
pub fn searched(id: u32, summary: &parvi_term::search::Summary) -> Vec<u8> {
    [
        &[SEARCHED][..],
        &id.to_le_bytes(),
        &summary.matches.to_le_bytes(),
        &summary.panes.to_le_bytes(),
        &[u8::from(summary.truncated)],
    ]
    .concat()
}

/// What became of a send: its error says whether the text went in.
pub fn outcome(result: &anyhow::Result<()>) -> Outcome {
    match result {
        Ok(()) => Outcome::Delivered,
        Err(e) if e.is::<parvi_term::engine::NotSubmitted>() => Outcome::Unknown,
        Err(e) if e.is::<parvi_term::engine::NotReady>() => Outcome::Unknown,
        Err(_) => Outcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;
    use crate::agent_events::Kind;
    use parvi_term::engine::{Activity, Facts};

    #[test]
    fn the_launcher_names_other_users_only_if_there_are_any() {
        let config = Config::default();
        assert_eq!(launcher(&config, &[], None)[1..], *config.json().as_bytes());
        let users = ["alice".into(), "bob".into()];
        let json = String::from_utf8(launcher(&config, &users, Some("no \"hooks\""))[1..].to_vec())
            .unwrap();
        assert!(
            json.ends_with(r#""rows":50,"users":["alice","bob"],"notifyOff":"no \"hooks\""}"#),
            "{json}"
        );
        serde_json::from_str::<serde_json::Value>(&json).unwrap();
    }

    #[test]
    fn encodes_sessions_as_json() {
        let frame = sessions(
            &[
                SessionInfo {
                    id: "$0".into(),
                    name: "say \"hi\"\\\n".into(),
                    panes: vec![(1, 80, 24), (2, 40, 24)],
                    seat: None,
                    facts: Some(Facts {
                        windows: 2,
                        foreign_clients: 1,
                        pinned: true,
                        mouse: false,
                        command: "claude".into(),
                        path: "/srv/my app".into(),
                    }),
                    activity: Activity {
                        active: false,
                        last_output: Some(UNIX_EPOCH + Duration::from_millis(1500)),
                    },
                    waiting: vec![(2, "Do you want to proceed?".into())],
                    user: Some("alice".into()),
                },
                SessionInfo {
                    id: "$1".into(),
                    name: "empty".into(),
                    panes: vec![],
                    seat: None,
                    facts: None,
                    activity: Activity::default(),
                    waiting: vec![],
                    user: None,
                },
            ],
            &HashMap::from([(
                "$0".to_owned(),
                Event {
                    kind: Kind::NeedsInput,
                    at: 9,
                    summary: "May I?".into(),
                    seen: false,
                },
            )]),
            1,
        );
        assert_eq!(frame[0], SESSIONS);
        assert_eq!(
            std::str::from_utf8(&frame[1..]).unwrap(),
            r#"[{"id":"$0","name":"say \"hi\"\\\u000a","user":"alice","panes":[{"id":1,"cols":80,"rows":24},{"id":2,"cols":40,"rows":24,"waiting":"Do you want to proceed?"}],"windows":2,"foreign":1,"pinned":true,"mouse":false,"command":"claude","cwd":"/srv/my app","active":false,"last":1500,"event":{"kind":"needs-input","at":9,"seen":false,"summary":"May I?"}},{"id":"$1","name":"empty","panes":[],"active":false}]"#
        );
    }

    #[test]
    fn marks_the_sizing_seat_for_its_holder_and_everyone_else() {
        let held = [SessionInfo {
            id: "$0".into(),
            name: "a".into(),
            panes: vec![],
            seat: Some(7),
            facts: None,
            activity: Activity::default(),
            waiting: vec![],
            user: None,
        }];
        let json =
            |me| String::from_utf8(sessions(&held, &HashMap::new(), me)[1..].to_vec()).unwrap();
        assert_eq!(
            json(7),
            r#"[{"id":"$0","name":"a","panes":[],"seat":"mine","active":false}]"#
        );
        assert_eq!(
            json(8),
            r#"[{"id":"$0","name":"a","panes":[],"seat":"other","active":false}]"#
        );
    }
}
