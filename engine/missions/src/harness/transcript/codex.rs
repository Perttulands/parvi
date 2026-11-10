use super::super::adapter::{Pointer, Turn, TurnState};
use super::{changed, matches_sentinel, native_pointer, s, text};
use parvi_term::waiting::harness::Harness;
use serde_json::Value;
use std::io;

pub(super) fn read_codex(rows: &[Value], pointer: &Pointer, sentinel: &str) -> io::Result<Turn> {
    let mut turn = Turn::default();
    let Some(first) = rows.first() else {
        return Ok(turn);
    };
    if s(first, "type") != "session_meta"
        || std::path::Path::new(s(&first["payload"], "cwd")) != pointer.cwd
    {
        return Ok(turn);
    }
    turn.session_id = s(&first["payload"], "id").into();
    if turn.session_id.is_empty() {
        return Err(changed("Codex transcript has no native session identity"));
    }
    let start = rows
        .iter()
        .rposition(|r| native_pointer(Harness::Codex, r, &pointer.text));
    let Some(start) = start else {
        return Ok(turn);
    };
    let mut context_id = String::new();
    let mut turn_id = String::new();
    let mut answer = String::new();
    for (index, r) in rows.iter().enumerate() {
        let kind = s(r, "type");
        let p = &r["payload"];
        if kind == "turn_context" {
            context_id = s(p, "turn_id").into();
            if index >= start
                && turn.consumed
                && !turn.model.is_empty()
                && (s(p, "model") != turn.model || s(p, "effort") != turn.effort)
            {
                return Err(changed("Codex model or effort changed"));
            }
            turn.model = s(p, "model").into();
            turn.effort = s(p, "effort").into();
            turn_id = context_id.clone();
        }
        if index < start {
            continue;
        }
        if native_pointer(Harness::Codex, r, &pointer.text) {
            turn.consumed = true;
            turn_id.clear();
            answer.clear();
            continue;
        }
        if kind == "response_item" && s(p, "type") == "message" {
            let role = s(p, "role");
            let kinds = &p["internal_chat_message_metadata_passthrough"]["content_item_kinds"];
            let has = |needle| {
                kinds
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == needle))
            };
            if role == "user" && (kinds.as_array().is_none_or(Vec::is_empty) || has("user.text")) {
                turn.operator_turns += 1;
            }
            if role == "developer" && has("model_switch.instructions") {
                return Err(changed("Codex model changed"));
            }
            if role == "assistant"
                && (s(p, "channel") == "final" || s(p, "phase") == "final_answer")
            {
                answer = text(&p["content"]);
                turn.state = TurnState::Pending;
            }
        }
        if kind == "event_msg" && s(p, "type") == "turn_aborted" {
            turn.operator_turns += 1;
            answer.clear();
        }
        if kind == "event_msg" && s(p, "type") == "task_complete" && !answer.is_empty() {
            if turn_id.is_empty() {
                turn_id = context_id.clone();
            }
            if s(p, "turn_id") != turn_id {
                continue;
            }
            if matches_sentinel(&answer, sentinel) {
                turn.state = TurnState::Completed { text: answer };
                return Ok(turn);
            }
            turn.state = TurnState::EndedWithoutSentinel;
        }
    }
    Ok(turn)
}
