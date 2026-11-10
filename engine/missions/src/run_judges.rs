//! Runs a gate's judge chain, reusing durable earlier reviews.
use crate::coordinator::Control;
use crate::run_job::execute_formation;
use crate::run_worker::event;
use crate::{Coordinator, Mission, RunStart};
use serde_json::json;
use std::io;

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_judges(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    req: &RunStart,
    workspace: &std::path::Path,
    control: &Control,
    inputs: &[crate::RunInput],
    gate: &crate::Gate,
    gate_attempt: i64,
    input: &crate::RoutedInput,
) -> io::Result<crate::JudgeVerdict> {
    let chain = crate::graph::judge_chain(mission, &gate.id);
    if chain.is_empty() {
        return Err(io::Error::other("judge chain is missing"));
    }
    let mut current = input.clone();
    let mut final_text = String::new();
    let mut final_node = String::new();
    let mut final_attempt = 0;
    let mut final_slot = String::new();
    let mut final_ports = Vec::new();
    let mut final_output: Option<crate::FormationResult> = None;
    let ledger = c.ledger(id)?.read()?;
    let anchor = ledger
        .iter()
        .rev()
        .find(|e| e.kind == "gate_evaluating" && e.gate_id == gate.id)
        .map(|e| e.seq)
        .unwrap_or(0);
    let invalidated = ledger.iter().any(|e| {
        e.seq > anchor
            && e.kind == "run_blocked"
            && e.gate_id == gate.id
            && e.data.get("code").and_then(|v| v.as_str()) == Some("invalid_judge_result")
    });
    let count = chain.len();
    let mut replaying = true;
    for (index, judge) in chain.into_iter().enumerate() {
        let events = c.ledger(id)?.read()?;
        let previous = if replaying && !(invalidated && index + 1 == count) {
            events.iter().rev().find(|e| {
                e.seq > anchor
                    && e.kind == "node_output"
                    && e.node_id == judge.id
                    && e.data.get("reason").and_then(|v| v.as_str()) == Some("judge")
            })
        } else {
            None
        };
        if let Some(output) = previous {
            final_ports = judge.outputs.iter().map(|port| port.id.clone()).collect();
            final_text = output
                .data
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into();
            final_node.clone_from(&judge.id);
            final_attempt = output.attempt;
            final_slot = judge
                .slots
                .first()
                .map(|s| s.id.clone())
                .unwrap_or_default();
            if let Some(port) = judge.outputs.first() {
                current.text = output
                    .data
                    .get("outputs")
                    .and_then(|v| v.get(&port.id))
                    .and_then(|v| v.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(&final_text)
                    .into();
            }
            continue;
        }
        replaying = false;
        let attempt = events
            .iter()
            .filter(|e| e.kind == "node_started" && e.node_id == judge.id)
            .map(|e| e.attempt)
            .max()
            .unwrap_or(0)
            .max(gate_attempt - 1)
            + 1;
        let result = execute_formation(
            c,
            id,
            mission,
            req,
            workspace,
            control,
            judge.id.clone(),
            attempt,
            vec![serde_json::to_value(&current)?],
            current.text.clone(),
            inputs,
            "judge",
            index + 1 == count,
            None,
            Vec::new(),
        )?;
        if index + 1 != count {
            record_output(c, id, req, workspace, control, &judge.id, attempt, &result)?;
        }
        final_text = result.text.clone();
        final_node.clone_from(&judge.id);
        final_ports = judge.outputs.iter().map(|port| port.id.clone()).collect();
        final_attempt = attempt;
        final_slot = judge
            .slots
            .first()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        if let Some(port) = judge.outputs.first() {
            current.text = result
                .outputs
                .get(&port.id)
                .and_then(|v| v.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or(&final_text)
                .into();
        }
        if index + 1 == count {
            final_output = Some(result);
        }
    }
    match crate::parse_verdict(&final_text) {
        Ok(verdict) => {
            if let Some(output) = &final_output {
                record_output(
                    c,
                    id,
                    req,
                    workspace,
                    control,
                    &final_node,
                    final_attempt,
                    output,
                )?;
            }
            Ok(verdict)
        }
        Err(error) => {
            let rejected = |text: &str| -> String {
                let marker = format!("```{}", crate::contract_names::VERDICT);
                text.lines()
                    .skip_while(|line| !line.trim().starts_with(&marker))
                    .skip(1)
                    .take_while(|line| line.trim() != "```")
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_owned()
            };
            let corrected = correct_judge(
                c,
                id,
                control,
                crate::JudgeCorrection {
                    run_id: id.into(),
                    node_id: final_node.clone(),
                    attempt: final_attempt,
                    rejected: final_text.clone(),
                    error: error.clone(),
                },
            )
            .map_err(|e| io::Error::other(format!("invalid_judge_result[{}]: {e}", gate.id)))?;
            let Some(corrected) = corrected else {
                if let Some(output) = &final_output {
                    record_output(
                        c,
                        id,
                        req,
                        workspace,
                        control,
                        &final_node,
                        final_attempt,
                        output,
                    )?;
                }
                let mut failed = event(
                    "judge_attempt_failed",
                    &gate.id,
                    gate_attempt,
                    json!({"code":"invalid_judge_result","reason":error,"returned":rejected(&final_text)}),
                );
                failed.gate_id = gate.id.clone();
                append(c, id, control, failed)?;
                return Err(io::Error::other(format!(
                    "invalid_judge_result[{}]: {error}",
                    gate.id
                )));
            };
            let mut failed = event(
                "judge_attempt_failed",
                &final_node,
                final_attempt,
                json!({"code":"invalid_judge_result","reason":error,"correction":true,"returned":rejected(&final_text)}),
            );
            failed.gate_id = gate.id.clone();
            failed.slot_id = final_slot.clone();
            append(c, id, control, failed)?;
            match crate::parse_verdict(&corrected) {
                Ok(verdict) => {
                    if let Some(mut output) = final_output {
                        let (_, payloads) = crate::parse_outputs(&corrected).map_err(|e| {
                            io::Error::other(format!("invalid_output_payloads: {e}"))
                        })?;
                        if payloads.len() != final_ports.len()
                            || final_ports.iter().any(|port| !payloads.contains_key(port))
                        {
                            return Err(io::Error::other(
                                "invalid_output_payloads: correction must contain all and only declared output ports",
                            ));
                        }
                        output.outputs = serde_json::to_value(payloads)?
                            .as_object()
                            .cloned()
                            .unwrap_or_default();
                        output.text = corrected;
                        record_output(
                            c,
                            id,
                            req,
                            workspace,
                            control,
                            &final_node,
                            final_attempt,
                            &output,
                        )?;
                    }
                    Ok(verdict)
                }
                Err(error) => {
                    let mut failed = event(
                        "judge_attempt_failed",
                        &gate.id,
                        gate_attempt,
                        json!({"code":"invalid_judge_result","reason":error,"returned":rejected(&corrected)}),
                    );
                    failed.gate_id = gate.id.clone();
                    append(c, id, control, failed)?;
                    Err(io::Error::other(format!(
                        "invalid_judge_result[{}]: {error}",
                        gate.id
                    )))
                }
            }
        }
    }
}

fn correct_judge(
    c: &Coordinator,
    id: &str,
    control: &Control,
    correction: crate::JudgeCorrection,
) -> Result<Option<String>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let executor = c.inner.executor.clone();
    let node = correction.node_id.clone();
    let attempt = correction.attempt;
    let prior = c
        .ledger(id)
        .and_then(|ledger| ledger.read())
        .map_err(|e| e.to_string())?;
    let worker = std::thread::spawn(move || {
        executor.correct_judge(correction, crate::ExecutionSink { sender: tx })
    });
    let mut failure = None;
    for pending in rx {
        let replayed = prior.iter().any(|old| {
            old.node_id == node
                && old.attempt == attempt
                && old.kind == pending.event.kind
                && old.data.get("dispatchId") == pending.event.data.get("dispatchId")
                && (pending.event.kind == "slot_dispatch"
                    || pending.event.data.get("seatEventSeq").is_some()
                        && old.data.get("seatEventSeq") == pending.event.data.get("seatEventSeq"))
        });
        if replayed {
            let _ = pending.ack.send(Ok(()));
            continue;
        }
        let mut record = event(
            &pending.event.kind,
            &node,
            attempt,
            serde_json::Value::Object(pending.event.data),
        );
        record.slot_id = pending.event.slot_id;
        let detached = record.kind == "seat_cleanup"
            && record.data.get("outcome").and_then(|value| value.as_str()) == Some("left_shutdown")
            && control.stopping.load(std::sync::atomic::Ordering::Acquire);
        let result = if detached {
            c.append(id, record).map_err(|e| e.to_string())
        } else if failure.is_none() {
            crate::run_worker::append_live(c, id, control, record)
                .map_err(|e| e.to_string())
                .and_then(|appended| {
                    if appended {
                        Ok(())
                    } else {
                        Err("run canceled".into())
                    }
                })
        } else {
            Err("ledger unavailable".into())
        };
        if let Err(error) = &result {
            failure = Some(error.clone());
        }
        let _ = pending.ack.send(result);
    }
    let result = worker
        .join()
        .map_err(|_| "judge correction panicked".to_string())?;
    if let Some(error) = failure {
        return Err(error);
    }
    result
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_output(
    c: &Coordinator,
    id: &str,
    req: &RunStart,
    workspace: &std::path::Path,
    control: &Control,
    node: &str,
    attempt: i64,
    result: &crate::FormationResult,
) -> io::Result<()> {
    crate::run_workspace::publish_output(
        c,
        id,
        req,
        workspace,
        control,
        event(
            "node_output",
            node,
            attempt,
            json!({"status":result.status,"text":result.text,"outputs":result.outputs,"reason":"judge","reportRef":format!("lab://{id}/{node}/report.md")}),
        ),
    )
}

fn append(c: &Coordinator, id: &str, control: &Control, event: crate::RunEvent) -> io::Result<()> {
    if crate::run_worker::append_live(c, id, control, event)? {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"))
    }
}
