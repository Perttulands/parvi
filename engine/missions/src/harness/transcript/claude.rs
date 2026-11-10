use super::super::adapter::{Pointer, Turn, TurnState};
use super::{changed, matches_sentinel, native_pointer, pointer_matches, s, text};
use parvi_term::waiting::harness::Harness;
use serde_json::Value;
use std::{collections::HashSet, io};

fn background_update(pending: &mut HashSet<String>, v: &Value) {
    if let Some(blocks) = v["message"]["content"].as_array() {
        for b in blocks {
            if s(v, "type") == "assistant"
                && s(b, "type") == "tool_use"
                && (b["input"]["run_in_background"] == true || s(b, "name") == "Monitor")
                && !s(b, "id").is_empty()
            {
                pending.insert(s(b, "id").to_owned());
            }
            if s(b, "type") == "tool_result" && b["is_error"] == true {
                pending.remove(s(b, "tool_use_id"));
            }
        }
    }
    let notices = [
        s(v, "content").to_owned(),
        s(&v["attachment"], "prompt").to_owned(),
        if s(v, "type") == "user" {
            text(&v["message"]["content"])
        } else {
            String::new()
        },
    ];
    for notice in notices {
        if !notice.trim_start().starts_with("<task-notification>") {
            continue;
        }
        let field = |key: &str| {
            notice
                .split_once(&format!("<{key}>"))
                .and_then(|(_, s)| s.split_once(&format!("</{key}>")))
                .map(|(s, _)| s.trim())
                .unwrap_or("")
        };
        if [
            "completed",
            "failed",
            "killed",
            "stopped",
            "cancelled",
            "canceled",
        ]
        .contains(&field("status"))
        {
            pending.remove(field("tool-use-id"));
        }
    }
}
pub(super) fn read_claude(rows: &[Value], pointer: &Pointer, sentinel: &str) -> io::Result<Turn> {
    let mut turn = Turn::default();
    let mut pending = HashSet::new();
    let start = rows
        .iter()
        .rposition(|r| native_pointer(Harness::Claude, r, &pointer.text));
    let Some(start) = start else {
        return Ok(turn);
    };
    for r in &rows[start..] {
        let kind = s(r, "type");
        let msg = &r["message"];
        let body = text(&msg["content"]);
        let cwd = s(r, "cwd");
        let sid = s(r, "sessionId");
        let mut human = kind == "user" && !body.is_empty() && r["isMeta"] != true;
        let origin = s(&r["origin"], "kind");
        let source = s(r, "promptSource");
        if human && (!origin.is_empty() || !source.is_empty()) {
            human = origin == "human" || (origin.is_empty() && source == "typed");
        }
        if human
            && pointer_matches(&body, &pointer.text)
            && std::path::Path::new(cwd) == pointer.cwd
            && !sid.is_empty()
        {
            turn = Turn {
                consumed: true,
                session_id: sid.into(),
                ..Turn::default()
            };
            pending.clear();
            continue;
        }
        if !turn.consumed {
            continue;
        }
        if !sid.is_empty() && sid != turn.session_id {
            return Err(changed("Claude native session identity changed"));
        }
        if kind == "user"
            && (body.contains("<command-name>/model</command-name>")
                || body.contains("<command-name>/effort</command-name>"))
        {
            return Err(changed("Claude model or effort changed"));
        }
        if human
            || (kind == "user" && body.starts_with("[Request interrupted by user"))
            || (kind == "queue-operation"
                && s(r, "operation") == "enqueue"
                && !s(r, "content").trim().starts_with("<task-notification>"))
            || (kind == "attachment"
                && s(&r["attachment"], "type") == "queued_command"
                && s(&r["attachment"]["origin"], "kind") == "human")
        {
            turn.operator_turns += 1;
        }
        if !s(r, "effort").is_empty() {
            turn.effort = s(r, "effort").into();
        }
        background_update(&mut pending, r);
        if turn.state == TurnState::BackgroundWork && pending.is_empty() {
            turn.state = TurnState::EndedWithoutSentinel;
        }
        if kind != "assistant" {
            continue;
        }
        let model = s(msg, "model");
        if !model.is_empty() && model != "<synthetic>" {
            if !turn.model.is_empty() && turn.model != model {
                return Err(changed("Claude model changed"));
            }
            turn.model = model.into();
        }
        if !s(msg, "effort").is_empty() {
            turn.effort = s(msg, "effort").into();
        }
        if body.is_empty() {
            continue;
        }
        turn.state = TurnState::Pending;
        let stop = s(msg, "stop_reason");
        if matches_sentinel(&body, sentinel) && (stop.is_empty() || stop == "end_turn") {
            turn.state = TurnState::Completed { text: body };
            return Ok(turn);
        }
        if stop == "end_turn" {
            turn.state = if pending.is_empty() {
                TurnState::EndedWithoutSentinel
            } else {
                TurnState::BackgroundWork
            };
        }
    }
    Ok(turn)
}
