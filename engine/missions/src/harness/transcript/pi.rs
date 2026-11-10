use super::super::adapter::{Pointer, Turn, TurnState};
use super::{changed, cwd_matches, matches_sentinel, native_pointer, s, text};
use parvi_term::waiting::harness::Harness;
use serde_json::Value;
use std::io;

fn model_name(provider: &str, model: &str) -> String {
    if provider.is_empty() || model.is_empty() {
        model.into()
    } else {
        format!("{provider}/{model}")
    }
}
pub(super) fn read_pi(rows: &[Value], pointer: &Pointer, sentinel: &str) -> io::Result<Turn> {
    let mut turn = Turn::default();
    let Some(first) = rows.first() else {
        return Ok(turn);
    };
    if s(first, "type") != "session" || !cwd_matches(s(first, "cwd"), &pointer.cwd) {
        return Ok(turn);
    }
    let sid = s(first, "id");
    if sid.is_empty() {
        return Err(changed("Pi session has no native session identity"));
    }
    let mut model = String::new();
    let mut effort = String::new();
    for r in rows.iter().skip(1) {
        match s(r, "type") {
            "model_change" => {
                let next = model_name(s(r, "provider"), s(r, "modelId"));
                if turn.consumed && !turn.model.is_empty() && next != turn.model {
                    return Err(changed("Pi model changed"));
                }
                model = next;
                if turn.consumed {
                    turn.model = model.clone();
                }
            }
            "thinking_level_change" => {
                let next = s(r, "thinkingLevel");
                if turn.consumed && !turn.effort.is_empty() && next != turn.effort {
                    return Err(changed("Pi effort changed"));
                }
                effort = next.into();
                if turn.consumed {
                    turn.effort = effort.clone();
                }
            }
            "message" if native_pointer(Harness::Pi, r, &pointer.text) => {
                turn = Turn {
                    consumed: true,
                    session_id: sid.into(),
                    model: model.clone(),
                    effort: effort.clone(),
                    ..Turn::default()
                };
            }
            "message" if turn.consumed => {
                let msg = &r["message"];
                match s(msg, "role") {
                    "user" => turn.operator_turns += 1,
                    "assistant" => {
                        if turn.model.is_empty() {
                            turn.model = model_name(s(msg, "provider"), s(msg, "model"));
                        }
                        let body = text(&msg["content"]);
                        turn.state = match s(msg, "stopReason") {
                            "stop" if matches_sentinel(&body, sentinel) => {
                                return Ok(Turn {
                                    state: TurnState::Completed { text: body },
                                    ..turn
                                });
                            }
                            "stop" | "length" | "error" => TurnState::EndedWithoutSentinel,
                            "aborted" => {
                                turn.operator_turns += 1;
                                TurnState::EndedWithoutSentinel
                            }
                            _ => TurnState::Pending,
                        };
                    }
                    _ => (),
                }
            }
            _ => (),
        }
    }
    Ok(turn)
}
