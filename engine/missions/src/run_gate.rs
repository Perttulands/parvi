//! Aggregate durable gate results and route or request a verdict.
use crate::coordinator::Control;
use crate::run_replay::Replay;
use crate::run_scheduler::Finished;
use crate::run_worker::event;
use crate::{Coordinator, Mission, RunEvent, RunInput, RunStart};
use serde_json::json;
use std::io;
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, mpsc},
};

pub(crate) struct GateContext<'a> {
    pub c: &'a Coordinator,
    pub id: &'a str,
    pub control: &'a Arc<Control>,
    pub mission: &'a Mission,
    pub req: &'a RunStart,
    pub workspace: &'a Path,
    pub inputs: &'a [RunInput],
    pub events: &'a [RunEvent],
    pub replay: &'a Replay,
    pub done: &'a mpsc::Sender<Finished>,
    pub wake: &'a mpsc::Sender<()>,
    pub drain_gates: &'a HashSet<String>,
}

pub(crate) enum GateAction {
    None,
    Acted,
    Failed(String, io::Error),
}

pub(crate) fn advance(
    context: GateContext<'_>,
    running: &mut HashSet<String>,
    stopped: bool,
) -> io::Result<GateAction> {
    let GateContext {
        c,
        id,
        control,
        mission,
        req,
        workspace,
        inputs,
        events,
        replay,
        done: done_tx,
        wake: wake_tx,
        drain_gates,
    } = context;
    for gate in &mission.gates {
        if stopped
            && !(gate.kinds.len() == 1 && gate.kinds[0] == "human")
            && !(drain_gates.contains(&gate.id) && replay.evaluating.contains_key(&gate.id))
        {
            continue;
        }
        if running.contains(&format!("gate:{}", gate.id))
            || (replay.picking.contains(&gate.id) && !replay.evaluating.contains_key(&gate.id))
        {
            continue;
        }
        let (input, attempt, anchor) = if let Some(open) = replay.evaluating.get(&gate.id) {
            let input =
                serde_json::from_value(open.data.get("inputRef").cloned().unwrap_or_default())
                    .map_err(io::Error::other)?;
            (
                input,
                events
                    .iter()
                    .filter(|e| {
                        e.kind == "gate_evaluating" && e.gate_id == gate.id && e.seq <= open.seq
                    })
                    .count() as i64,
                open.seq,
            )
        } else if let Some(input) = replay.queue.get(&gate.id).and_then(|q| q.front()) {
            let attempt = events
                .iter()
                .filter(|e| e.kind == "gate_evaluating" && e.gate_id == gate.id)
                .count() as i64
                + 1;
            let mut evaluating = event("gate_evaluating", &gate.id, 0, json!({"inputRef":input}));
            evaluating.gate_id = gate.id.clone();
            append(c, id, control, evaluating)?;
            (input.clone(), attempt, events.len() as u64 + 1)
        } else {
            continue;
        };
        let prior_code = events.iter().find(|e| {
            e.seq > anchor
                && e.kind == "gate_kind_result"
                && e.gate_id == gate.id
                && e.data.get("kind").and_then(|v| v.as_str()) == Some("code")
        });
        let prior_judge = events.iter().find(|e| {
            e.seq > anchor
                && e.kind == "gate_kind_result"
                && e.gate_id == gate.id
                && e.data.get("kind").and_then(|v| v.as_str()) == Some("formation")
        });
        let result = if gate.kinds.iter().any(|kind| kind == "code") {
            let checked = (|| -> Result<(crate::JudgeVerdict, serde_json::Value), String> {
                let source = std::fs::read_to_string(
                    c.inner
                        .root
                        .join("runs")
                        .join(format!("{id}.bindings.toml")),
                )
                .map_err(|e| e.to_string())?;
                let bindings: crate::BindingsSnapshot =
                    toml::from_str(&source).map_err(|e| e.to_string())?;
                let binding = bindings.gate_binding(id, mission, gate)?;
                crate::code_binding::validate_results(id, mission, &bindings, events)?;
                let result =
                    crate::GateEvaluator::evaluate(&crate::CodeGateEvaluator, gate, &input)?;
                let data = binding.result_data(&input.text, &result)?;
                Ok((result, data))
            })();
            match checked {
                Ok((result, mut data)) => {
                    if prior_code.is_none() {
                        data["inputRef"] =
                            serde_json::to_value(&input).map_err(io::Error::other)?;
                        let mut event = event("gate_kind_result", &gate.id, 0, data);
                        event.gate_id = gate.id.clone();
                        append(c, id, control, event)?;
                    }
                    Some(result)
                }
                Err(error) => {
                    let error = io::Error::other(format!("gate_evaluator_error: {error}"));
                    return Ok(GateAction::Failed(gate.id.clone(), error));
                }
            }
        } else {
            None
        };
        if result.as_ref().is_none_or(|r| r.verdict != "fail")
            && gate.kinds.iter().any(|k| k == "formation")
        {
            if prior_judge.is_none()
                && let Some(resumed) = events.iter().rev().find(|event| {
                    event.seq > anchor
                        && event.kind == "run_resumed"
                        && event.data.get("judgeVerdict").and_then(|v| v.as_str()) == Some("fail")
                        && event.data.get("blockedGateId").and_then(|v| v.as_str())
                            == Some(gate.id.as_str())
                })
            {
                let block = events.iter().rev().find(|event| {
                    event.seq < resumed.seq
                        && event.kind == "run_blocked"
                        && event.gate_id == gate.id
                });
                let detail = block
                    .and_then(|event| event.data.get("reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("invalid judge result");
                let detail = detail
                    .strip_prefix("invalid_judge_result[")
                    .and_then(|s| {
                        s.split_once("]: ")
                            .map(|(_, reason)| format!("invalid judge result: {reason}"))
                    })
                    .unwrap_or_else(|| detail.to_owned());
                let result = crate::JudgeVerdict {
                    verdict: "fail".into(),
                    reason: format!(
                        "taken as a fail by {}: the judge's verdict was rejected ({detail})",
                        resumed.actor
                    ),
                    evidence: Vec::new(),
                };
                let mut kind = event(
                    "gate_kind_result",
                    &gate.id,
                    0,
                    json!({"kind":"formation","verdict":"fail","reason":result.reason,"evidence":[],"inputRef":input}),
                );
                kind.gate_id = gate.id.clone();
                append(c, id, control, kind)?;
                settle_gate(
                    c,
                    id,
                    control,
                    mission,
                    GateSettlement {
                        gate,
                        attempt,
                        input: &input,
                        result: &result,
                    },
                )?;
                return Ok(GateAction::Acted);
            }
            if let Some(prior) = prior_judge {
                let result = crate::JudgeVerdict {
                    verdict: prior
                        .data
                        .get("verdict")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .into(),
                    reason: prior
                        .data
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .into(),
                    evidence: prior
                        .data
                        .get("evidence")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.get("text").and_then(|v| v.as_str()).map(str::to_owned))
                        .collect(),
                };
                settle_gate(
                    c,
                    id,
                    control,
                    mission,
                    GateSettlement {
                        gate,
                        attempt,
                        input: &input,
                        result: &result,
                    },
                )?;
                return Ok(GateAction::Acted);
            }
            running.insert(format!("gate:{}", gate.id));
            let (c, id, mission, req, workspace, control, inputs, done, wake, gate, input) = (
                c.clone(),
                id.to_owned(),
                mission.clone(),
                req.clone(),
                workspace.to_path_buf(),
                control.clone(),
                inputs.to_vec(),
                done_tx.clone(),
                wake_tx.clone(),
                gate.clone(),
                input.clone(),
            );
            std::thread::spawn(move || {
                let result = crate::run_judges::run_judges(
                    &c, &id, &mission, &req, &workspace, &control, &inputs, &gate, attempt, &input,
                );
                let _ = done.send(Finished::Judge(
                    Box::new(gate),
                    attempt,
                    Box::new(input),
                    result,
                ));
                let _ = wake.send(());
            });
            return Ok(GateAction::Acted);
        }
        let result = result.unwrap_or_else(|| crate::JudgeVerdict {
            verdict: "pass".into(),
            reason: String::new(),
            evidence: Vec::new(),
        });
        settle_gate(
            c,
            id,
            control,
            mission,
            GateSettlement {
                gate,
                attempt,
                input: &input,
                result: &result,
            },
        )?;
        return Ok(GateAction::Acted);
    }
    Ok(GateAction::None)
}

pub(crate) struct GateSettlement<'a> {
    pub gate: &'a crate::Gate,
    pub attempt: i64,
    pub input: &'a crate::RoutedInput,
    pub result: &'a crate::JudgeVerdict,
}

pub(crate) fn settle_gate(
    c: &Coordinator,
    id: &str,
    control: &Control,
    mission: &Mission,
    settlement: GateSettlement<'_>,
) -> io::Result<()> {
    let GateSettlement {
        gate,
        attempt,
        input,
        result,
    } = settlement;
    let kind = if result.verdict == "pass" && gate.kinds.iter().any(|k| k == "human") {
        "human_input_requested"
    } else {
        "gate_verdict"
    };
    let events = c.ledger(id)?.read()?;
    let anchor = events
        .iter()
        .rev()
        .find(|event| event.kind == "gate_evaluating" && event.gate_id == gate.id)
        .map(|event| event.seq)
        .unwrap_or(0);
    let results: Vec<_> = events
        .iter()
        .filter(|event| {
            event.seq > anchor && event.kind == "gate_kind_result" && event.gate_id == gate.id
        })
        .collect();
    let evidence: Vec<_> = results
        .iter()
        .flat_map(|event| {
            event
                .data
                .get("evidence")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .cloned()
        })
        .collect();
    let mut sequences = serde_json::Map::new();
    let mut per_kind = serde_json::Map::new();
    let mut data = json!({"inputRef":input,"evidence":evidence});
    for entry in results {
        let name = entry
            .data
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        sequences.insert(name.into(), entry.seq.into());
        per_kind.insert(
            name.into(),
            entry.data.get("verdict").cloned().unwrap_or_default(),
        );
        if name == "code" {
            data["codeResultSeq"] = entry.seq.into();
            for (source, target) in [
                ("verdict", "codeVerdict"),
                ("reason", "codeReason"),
                ("resultEncoding", "resultEncoding"),
                ("resultSha256", "resultSha256"),
                ("gateBindingId", "gateBindingId"),
            ] {
                if let Some(value) = entry.data.get(source) {
                    data[target] = value.clone();
                }
            }
            data["codePerKind"] = json!({"code":entry.data.get("verdict")});
        }
    }
    if !sequences.is_empty() {
        data["kindResultSeqs"] = sequences.into();
    }
    if kind == "human_input_requested" {
        data["choices"] = json!(["pass", "fail"]);
        data["nodeId"] = gate.id.clone().into();
        data["gateId"] = gate.id.clone().into();
        data["requestedBy"] = gate.id.clone().into();
        data["prompt"] = "".into();
    } else {
        let mut failed = false;
        for name in ["code", "formation", "human"] {
            if !gate.kinds.iter().any(|kind| kind == name) {
                continue;
            }
            if failed {
                per_kind.insert(name.into(), "not_run".into());
            } else if per_kind.get(name).and_then(|v| v.as_str()) == Some("fail") {
                failed = true;
            }
        }
        data["perKind"] = per_kind.into();
        data["routePort"] = result.verdict.clone().into();
        data["verdict"] = result.verdict.clone().into();
        data["reason"] = result.reason.clone().into();
        data["routedEdges"] = crate::graph::outgoing_from_port(mission, &gate.id, &result.verdict)
            .map(|edge| edge.id.clone())
            .collect::<Vec<_>>()
            .into();
    }
    let mut e = event(
        kind,
        &gate.id,
        if kind == "gate_verdict" { attempt } else { 0 },
        data,
    );
    e.gate_id = gate.id.clone();
    append(c, id, control, e)
}

fn append(c: &Coordinator, id: &str, control: &Control, event: crate::RunEvent) -> io::Result<()> {
    crate::run_worker::append_live(c, id, control, event).map(|_| ())
}
