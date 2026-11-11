//! Validate human requests and their recorded decisions before routing.
use crate::RunEvent;
use std::{
    collections::{BTreeMap, HashSet},
    io,
};

pub(crate) fn validate(events: &[RunEvent]) -> io::Result<()> {
    let mut pending = BTreeMap::<&str, u64>::new();
    let mut routed = HashSet::new();
    for event in events {
        match event.kind.as_str() {
            "gate_evaluating" => {
                pending.remove(event.gate_id.as_str());
            }
            "human_input_requested" => {
                pending.insert(&event.gate_id, event.seq);
            }
            "human_verdict_recorded" => {
                let seq = event
                    .data
                    .get("requestedSeq")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                let verdict = event
                    .data
                    .get("verdict")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if pending.remove(event.gate_id.as_str()) != Some(seq)
                    || !["pass", "fail"].contains(&verdict)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "human verdict does not answer the pending request",
                    ));
                }
            }
            "gate_verdict" => {
                let Some(seq) = event
                    .data
                    .get("requestedSeq")
                    .and_then(|value| value.as_u64())
                    .filter(|seq| *seq > 0)
                else {
                    continue;
                };
                if !routed.insert(seq) || seq >= event.seq {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "human request was routed more than once or out of order",
                    ));
                }
                let request = events
                    .iter()
                    .find(|entry| {
                        entry.seq == seq
                            && entry.kind == "human_input_requested"
                            && entry.gate_id == event.gate_id
                    })
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "human verdict names an invalid request",
                        )
                    })?;
                let answer = events
                    .iter()
                    .find(|entry| {
                        entry.seq > seq
                            && entry.seq < event.seq
                            && entry.kind == "human_verdict_recorded"
                            && entry.gate_id == event.gate_id
                            && entry
                                .data
                                .get("requestedSeq")
                                .and_then(|value| value.as_u64())
                                == Some(seq)
                    })
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "human gate route has no recorded decision",
                        )
                    })?;
                // A pick carries the picked attempt's input, not the request's.
                if (!crate::run_pick::is_pick(event)
                    && event.data.get("inputRef") != request.data.get("inputRef"))
                    || event.data.get("verdict") != answer.data.get("verdict")
                    || event.data.get("reason") != answer.data.get("reason")
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "human gate route contradicts its recorded request or decision",
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn route(
    c: &crate::Coordinator,
    id: &str,
    control: &crate::coordinator::Control,
    mission: &parvi_mission_model::Mission,
    events: &[RunEvent],
) -> io::Result<bool> {
    use crate::run_worker::event;
    use serde_json::json;
    for answer in events.iter().filter(|e| e.kind == "human_verdict_recorded") {
        let seq = answer
            .data
            .get("requestedSeq")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if events.iter().any(|e| {
            e.kind == "gate_verdict"
                && e.gate_id == answer.gate_id
                && e.data.get("requestedSeq").and_then(|v| v.as_u64()) == Some(seq)
        }) {
            continue;
        }
        let Some(request) = events.iter().find(|e| {
            e.seq == seq && e.kind == "human_input_requested" && e.gate_id == answer.gate_id
        }) else {
            continue;
        };
        if crate::run_pick::is_pick(request) {
            crate::run_pick::route_human(c, id, control, mission, events, request, answer)?;
            return Ok(true);
        }
        let verdict = answer
            .data
            .get("verdict")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let mut data = json!({"routePort":verdict,"verdict":verdict,"reason":answer.data.get("reason"),"evidence":request.data.get("evidence"),"requestedSeq":seq,"inputRef":request.data.get("inputRef")});
        for name in [
            "kindResultSeqs",
            "codeResultSeq",
            "codeVerdict",
            "codeReason",
            "resultEncoding",
            "resultSha256",
            "gateBindingId",
        ] {
            if let Some(value) = request.data.get(name) {
                data[name] = value.clone();
            }
        }
        let mut per_kind = serde_json::Map::new();
        if let Some(sequences) = request
            .data
            .get("kindResultSeqs")
            .and_then(|v| v.as_object())
        {
            for (name, sequence) in sequences {
                if let Some(result) = events
                    .iter()
                    .find(|event| Some(event.seq) == sequence.as_u64())
                {
                    per_kind.insert(
                        name.clone(),
                        result.data.get("verdict").cloned().unwrap_or_default(),
                    );
                }
            }
        }
        per_kind.insert("human".into(), verdict.into());
        data["perKind"] = per_kind.into();
        data["routedEdges"] = crate::graph::outgoing_from_port(mission, &answer.gate_id, verdict)
            .map(|edge| edge.id.clone())
            .collect::<Vec<_>>()
            .into();
        let mut e = event(
            "gate_verdict",
            &answer.gate_id,
            events
                .iter()
                .filter(|e| {
                    e.kind == "gate_evaluating"
                        && e.gate_id == answer.gate_id
                        && e.seq <= request.seq
                })
                .count() as i64,
            data,
        );
        e.gate_id = answer.gate_id.clone();
        e.actor = answer.actor.clone();
        crate::run_worker::append_live(c, id, control, e)?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn event(seq: u64, kind: &str, data: serde_json::Value) -> RunEvent {
        RunEvent {
            seq,
            kind: kind.into(),
            gate_id: "review".into(),
            data: data.as_object().unwrap().clone(),
            ..Default::default()
        }
    }
    #[test]
    fn a_route_must_match_the_exact_request_and_recorded_response() {
        let request = event(
            1,
            "human_input_requested",
            json!({"inputRef":{"text":"approved"}}),
        );
        let answer = event(
            2,
            "human_verdict_recorded",
            json!({"requestedSeq":1,"verdict":"pass","reason":"  yes\n"}),
        );
        let route = event(
            3,
            "gate_verdict",
            json!({"requestedSeq":1,"verdict":"pass","reason":"  yes\n","inputRef":{"text":"approved"}}),
        );
        assert!(validate(&[request.clone(), answer.clone(), route.clone()]).is_ok());
        let mut altered = route;
        altered.data.insert("reason".into(), "yes".into());
        assert!(validate(&[request.clone(), answer, altered]).is_err());
        let newer = event(2, "gate_evaluating", json!({}));
        let stale = event(
            3,
            "human_verdict_recorded",
            json!({"requestedSeq":1,"verdict":"pass"}),
        );
        assert!(validate(&[request, newer, stale]).is_err());
    }
}
