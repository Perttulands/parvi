//! Check gate evaluation boundaries while rebuilding a run.
use crate::RunEvent;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

pub(crate) fn validate(events: &[RunEvent]) -> io::Result<()> {
    let mut active = BTreeMap::<&str, BTreeSet<&str>>::new();
    for event in events {
        match event.kind.as_str() {
            "gate_evaluating"
                if event.gate_id.is_empty()
                    || active.insert(&event.gate_id, BTreeSet::new()).is_some() =>
            {
                return Err(invalid("a gate began a second open evaluation"));
            }
            "gate_kind_result" => {
                let kinds = active
                    .get_mut(event.gate_id.as_str())
                    .ok_or_else(|| invalid("a gate kind result has no open evaluation"))?;
                let kind = event
                    .data
                    .get("kind")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if !["code", "formation"].contains(&kind) || !kinds.insert(kind) {
                    return Err(invalid(
                        "a gate kind was recorded more than once or is unknown",
                    ));
                }
            }
            // A pick judges no open evaluation: it answers its pick_started.
            "human_input_requested" | "gate_verdict" if crate::run_pick::is_pick(event) => {}
            "human_input_requested" if active.remove(event.gate_id.as_str()).is_none() => {
                return Err(invalid("a human request has no open evaluation"));
            }
            "gate_verdict"
                if event
                    .data
                    .get("requestedSeq")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0)
                    == 0
                    && active.remove(event.gate_id.as_str()).is_none() =>
            {
                return Err(invalid("a gate verdict has no open evaluation"));
            }
            _ => {}
        }
    }
    Ok(())
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn event(kind: &str, data: serde_json::Value) -> RunEvent {
        RunEvent {
            kind: kind.into(),
            gate_id: "review".into(),
            data: data.as_object().unwrap().clone(),
            ..Default::default()
        }
    }
    #[test]
    fn kind_results_and_automatic_verdicts_cannot_be_delivered_twice() {
        let start = event("gate_evaluating", json!({}));
        let code = event("gate_kind_result", json!({"kind":"code"}));
        let verdict = event("gate_verdict", json!({"verdict":"pass"}));
        assert!(validate(&[start.clone(), code.clone(), verdict.clone()]).is_ok());
        assert!(validate(&[start.clone(), code.clone(), code, verdict.clone()]).is_err());
        assert!(validate(&[start, verdict.clone(), verdict]).is_err());
    }
}
