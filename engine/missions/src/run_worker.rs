//! Drives an admitted run and is the sole writer of executor events.
use crate::coordinator::Control;
use crate::run_storage::now;

pub(crate) enum Outcome {
    Succeeded(Vec<String>),
    Rejected {
        end_id: String,
        gate_id: String,
        reason: String,
        actor: String,
        end_ids: Vec<String>,
    },
    Escalated(String),
    Starved(Vec<(String, String, Vec<String>)>),
    Unfinished(Vec<String>),
    /// A spent Limit card stopped the run.
    Limit(crate::run_limit::LimitStop),
    Stopped,
}
use crate::{BindingsSnapshot, Coordinator, RunEvent, render_run_inputs, resolve_run_inputs};
use parvi_mission_model::{Mission, RunStart};
use serde_json::json;
use std::{
    io,
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

pub fn execute(
    c: Coordinator,
    id: String,
    mission: Mission,
    _bindings: BindingsSnapshot,
    req: RunStart,
    workspace: PathBuf,
    control: Arc<Control>,
) {
    let result = execute_inner(&c, &id, &mission, &req, workspace.clone(), &control);
    let cleanup = if control.stopping.load(Ordering::Acquire) {
        Ok(())
    } else {
        crate::run_workspace_recovery::cleanup(&c, &id, &req, &workspace, &control)
    };
    let mut result = match (result, cleanup) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(io::Error::other(format!(
            "{error}; workspace cleanup failed: {cleanup}"
        ))),
    };
    if !control.stopping.load(Ordering::Acquire)
        && (control.canceled.load(Ordering::Acquire)
            || matches!(result, Ok(Outcome::Succeeded(_) | Outcome::Rejected { .. })))
        && let Err(error) = crate::run_left_seats::end(&c, &id, None, "run_final")
    {
        result = Err(error);
        control.canceled.store(false, Ordering::Release);
    }
    let mut done = control.done.lock().unwrap();
    let canceled = control.canceled.load(Ordering::Acquire);
    let stopping = control.stopping.load(Ordering::Acquire);
    if stopping {
        let events = c
            .ledger(&id)
            .and_then(|ledger| ledger.read().map_err(crate::error::to_io));
        let waiting = events
            .as_ref()
            .ok()
            .and_then(|events| {
                crate::project_run(events, &id).ok().map(|projection| {
                    crate::run_recovery::only_waiting(&mission, events, &projection)
                })
            })
            .unwrap_or(false);
        if !waiting {
            let open = events
                .as_deref()
                .map(crate::run_open_dispatch::unresolved)
                .unwrap_or_default();
            if let Err(e) = c.append(&id, RunEvent { kind:"run_blocked".into(), timestamp:now(), data:json!({"code":"coordinator_shutdown","reason":"coordinator shut down; resume to continue","resumeAllowed":true,"resumePolicy":"explicit","openDispatches":open.iter().map(|e| &e.data).collect::<Vec<_>>()}).as_object().unwrap().clone(), ..Default::default() }) { eprintln!("parvi: cannot record shutdown of run {id}: {e}"); }
        }
        *done = true;
        control.finished.notify_all();
        drop(done);
        c.inner.controls.lock().unwrap().remove(&id);
        return;
    }
    let event = if canceled {
        RunEvent {
            kind: "run_canceled".into(),
            actor: control.actor.lock().unwrap().clone(),
            data: json!({"reason":control.reason.lock().unwrap().clone(),"final":true})
                .as_object()
                .unwrap()
                .clone(),
            timestamp: now(),
            ..Default::default()
        }
    } else {
        match result {
            Ok(Outcome::Succeeded(end_ids)) => match if req.formation_id.is_empty() {
                c.ledger(&id).map_err(crate::outputs::MissingOutput::Ledger).and_then(|ledger| crate::outputs::resolve_with_picks(&ledger, &mission))
            } else { Ok(crate::outputs::Resolved { outputs: Vec::new(), picks: Vec::new() }) } {
                Ok(resolved) => success_event(end_ids, resolved),
                Err(crate::outputs::MissingOutput::Missing(reason)) => RunEvent { kind: "run_failed".into(), actor: "archond".into(), timestamp: now(), data: json!({"final":true,"endIds":end_ids,"code":"missing_output","reason":reason}).as_object().unwrap().clone(), ..Default::default() },
                Err(crate::outputs::MissingOutput::Ledger(error)) => { eprintln!("parvi: cannot resolve run {id} outputs: {error}"); RunEvent { kind:"run_blocked".into(), timestamp:now(), data:json!({"code":"executor_failed","reason":error.to_string(),"resumeAllowed":true}).as_object().unwrap().clone(), ..Default::default() } }
            },
            Ok(Outcome::Rejected { end_id, gate_id, reason, actor, end_ids }) => RunEvent {
                kind: "run_failed".into(),
                timestamp: now(),
                actor,
                node_id: end_id.clone(),
                gate_id: gate_id.clone(),
                data: json!({"final":true,"endId":end_id,"gateId":gate_id,"reason":reason,"endIds":end_ids,"code":"path_rejected"})
                    .as_object()
                    .unwrap()
                    .clone(),
                ..Default::default()
            },
            Ok(Outcome::Escalated(node_id)) => RunEvent {
                kind: "run_blocked".into(),
                node_id,
                timestamp: now(),
                data: json!({"reason":"escalation","resumeAllowed":true})
                    .as_object()
                    .unwrap()
                    .clone(),
                ..Default::default()
            },
            Ok(Outcome::Starved(nodes)) => {
                let (node, _, missing) = &nodes[0];
                let waiting: Vec<_> = nodes.iter().map(|(id, title, missing)| json!({"nodeId":id,"title":title,"missingInputs":missing})).collect();
                RunEvent { kind: "run_blocked".into(), node_id: node.clone(), timestamp: now(), data: json!({"code":"reachable_node_starved","reason":format!("formation {node:?} is waiting on inputs {missing:?} that no upstream node produces; wire a producer to those ports and start a new run"),"blockedNodeId":node,"blockedGateId":"","boundary":"wiring","waitingNodes":waiting,"recoverable":false,"resumeAllowed":false,"resumePolicy":"authoring","openDispatches":[],"nextEpoch":1}).as_object().unwrap().clone(), ..Default::default() }
            },
            Ok(Outcome::Unfinished(nodes)) => {
                let reason = format!("run has unfinished work at {nodes:?}; resume to continue it");
                if let Err(error) = c.append(&id, RunEvent { kind: "error".into(), node_id: nodes[0].clone(), timestamp: now(), data: json!({"code":"run_work_unfinished","message":reason,"reason":reason,"boundary":"engine","nodeId":nodes[0],"recoverable":true}).as_object().unwrap().clone(), ..Default::default() }) { eprintln!("parvi: cannot record run {id} unfinished work: {error}"); }
                RunEvent { kind: "run_blocked".into(), node_id: nodes[0].clone(), timestamp: now(), data: json!({"code":"run_work_unfinished","reason":reason,"blockedNodeId":nodes[0],"resumeAllowed":true,"resumePolicy":"explicit"}).as_object().unwrap().clone(), ..Default::default() }
            },
            Ok(Outcome::Limit(stop)) => match crate::run_limit::final_event(&c, &id, &mission, &stop) {
                Ok(event) => event,
                Err(error) => RunEvent { kind:"run_blocked".into(), node_id: stop.node.clone(), timestamp:now(), data:json!({"code":"executor_failed","reason":error.to_string(),"resumeAllowed":true,"resumePolicy":"explicit"}).as_object().unwrap().clone(), ..Default::default() },
            },
            Ok(Outcome::Stopped) => unreachable!("stopped only during shutdown"),
            Err(e) => {
                let detail = e.to_string();
                let code = error_code(&detail);
                let gate_id = detail.strip_prefix("invalid_judge_result[").or_else(|| detail.strip_prefix("invalid_judge_pick[")).and_then(|s| s.split_once(']')).map(|(gate, _)| gate).unwrap_or("").to_owned();
                let already_recorded = c.ledger(&id).and_then(|ledger| ledger.read().map_err(crate::error::to_io)).is_ok_and(|events| events.iter().rev().take_while(|event| event.kind != "run_resumed" && event.kind != "run_started").any(|event| event.kind == "error" && event.data.get("detail").and_then(|value| value.as_str()) == Some(&detail)));
                let mut blocked_node = if gate_id.is_empty() { c.ledger(&id).and_then(|ledger| ledger.read().map_err(crate::error::to_io)).ok().and_then(|events| events.into_iter().rev().find(|event| event.kind == "error" && event.data.get("detail").and_then(|value| value.as_str()) == Some(&detail))).map(|event| event.node_id).unwrap_or_default() } else { gate_id.clone() };
                if !already_recorded && !["invalid_judge_result", "invalid_judge_pick", "reachable_node_starved", "run_work_unfinished"].contains(&code) && let Err(error) = c.append(
                    &id,
                    RunEvent {
                        kind: "error".into(),
                        timestamp: now(),
                        data: json!({"code":code,"detail":detail})
                            .as_object()
                            .unwrap()
                            .clone(),
                        ..Default::default()
                    },
                ) {
                    eprintln!("parvi: cannot record run {id} error: {error}");
                }
                let pending = if ["output_not_kept", "invalid_output_ref", "unavailable_output_ref"].contains(&code) {
                    c.ledger(&id).and_then(|ledger| ledger.read().map_err(crate::error::to_io)).ok().and_then(|events| {
                        events.iter().rev().find(|event| event.kind == "output_capture_pending"
                            && (blocked_node.is_empty() || event.node_id == blocked_node)
                            && !crate::run_capture::discarded(&events, event)
                            && !events.iter().any(|e| (e.kind == "node_output" || e.kind == "node_output_pending") && e.node_id == event.node_id && e.attempt == event.attempt)).cloned()
                    }).map(|event| {
                        if blocked_node.is_empty() { blocked_node.clone_from(&event.node_id); }
                        json!({"nodeId":event.node_id,"attempt":event.attempt,"output":event.data})
                    })
                } else { None };
                let mut data = json!({"code":code,"reason":detail,"detail":e.to_string(),"blockedNodeId":blocked_node,"blockedGateId":gate_id,"resumeAllowed":true,"resumePolicy":"explicit"}).as_object().unwrap().clone();
                if let Some(pending) = pending { data.insert("pendingOutput".into(), pending); }
                let open: Vec<_> = c.ledger(&id).and_then(|ledger| ledger.read().map_err(crate::error::to_io)).ok().map(|events| crate::run_open_dispatch::unresolved(&events).into_iter().filter(|event| event.node_id == blocked_node && !crate::run_open_dispatch::failed_completion(&events, event)).map(|event| event.data).collect()).unwrap_or_default();
                data.insert("openDispatches".into(), json!(open));
                RunEvent {
                    kind: "run_blocked".into(),
                    gate_id: gate_id.clone(),
                    node_id: blocked_node.clone(),
                    timestamp: now(),
                    data,
                    ..Default::default()
                }
            }
        }
    };
    if let Err(e) = c.append(&id, event) {
        eprintln!("parvi: cannot record run {id} outcome: {e}");
    }
    *done = true;
    control.finished.notify_all();
    drop(done);
    c.inner.controls.lock().unwrap().remove(&id);
    if let Ok(run) = c.run(&id) {
        c.changed(&id, run.event_count as u64);
    }
}
fn success_event(end_ids: Vec<String>, resolved: crate::outputs::Resolved) -> RunEvent {
    RunEvent {
        kind: if resolved.picks.is_empty() {
            "run_succeeded"
        } else {
            "run_picked"
        }
        .into(),
        timestamp: now(),
        data:
            json!({"final":true,"endIds":end_ids,"outputs":resolved.outputs,"picks":resolved.picks})
                .as_object()
                .unwrap()
                .clone(),
        ..Default::default()
    }
}

pub(crate) fn error_code(detail: &str) -> &'static str {
    [
        "persona_snapshot_invalid",
        "unknown_dispatch",
        "completion_sentinel_timeout",
        "seat_idle_without_sentinel",
        "seat_stuck",
        "missing_output_payload",
        "invalid_output_payloads",
        "invalid_output_ref",
        "unavailable_output_ref",
        "oversized_output_ref",
        "output_not_kept",
        "invalid_judge_result",
        "invalid_judge_pick",
        "gate_evaluator_error",
        "reachable_node_starved",
        "run_work_unfinished",
        "run_finalization_interrupted",
    ]
    .into_iter()
    .find(|code| detail.contains(code))
    .or_else(|| agent_status(detail))
    .unwrap_or("executor_failed")
}
/// An agent that ended its turn with a done line other than ok: it said
/// itself that the step is blocked, or failed.
fn agent_status(detail: &str) -> Option<&'static str> {
    let done = &detail[detail.find("<<<PARVI-DONE")?..];
    let line = done.lines().next().unwrap_or(done);
    Some(if line.contains("status=blocked") {
        "agent_blocked"
    } else {
        "agent_failed"
    })
}
pub(crate) fn event(kind: &str, node: &str, attempt: i64, data: serde_json::Value) -> RunEvent {
    RunEvent {
        kind: kind.into(),
        node_id: node.into(),
        attempt,
        timestamp: now(),
        data: data.as_object().unwrap().clone(),
        ..Default::default()
    }
}
/// Serialize worker writes with an abort request, so no new events land once
/// cancellation is accepted. Executors still drain and perform cleanup.
pub(crate) fn append_live(
    c: &Coordinator,
    id: &str,
    control: &Control,
    event: RunEvent,
) -> io::Result<bool> {
    let _guard = control.done.lock().unwrap();
    if control.canceled.load(Ordering::Acquire)
        || (matches!(event.kind.as_str(), "node_started" | "slot_dispatch")
            && control.stopping.load(Ordering::Acquire))
    {
        return Ok(false);
    }
    c.append(id, event)?;
    Ok(true)
}
fn execute_inner(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    req: &RunStart,
    workspace: PathBuf,
    control: &Arc<Control>,
) -> io::Result<Outcome> {
    crate::run_capture::recover(c, id, mission, req, &workspace, control)?;
    crate::run_workspace_recovery::recover_pending(c, id, req, &workspace, control)?;
    if req.formation_id.is_empty() {
        return crate::run_scheduler::schedule(c, id, mission, req, workspace, control);
    }
    let events = c.ledger(id)?.read().map_err(crate::error::to_io)?;
    if events
        .iter()
        .rev()
        .find(|e| {
            e.node_id == req.formation_id && (e.kind == "node_output" || e.kind == "node_started")
        })
        .is_some_and(|e| {
            e.kind == "node_output"
                && mission
                    .formation(&req.formation_id)
                    .is_some_and(|formation| {
                        formation.outputs.iter().all(|port| {
                            e.data
                                .get("outputs")
                                .and_then(|v| v.get(&port.id))
                                .is_some()
                        })
                    })
        })
    {
        return Ok(Outcome::Succeeded(Vec::new()));
    }
    let inputs = resolve_run_inputs(mission, &req.inputs);
    let input = render_run_inputs(mission, &inputs);
    let node = req.formation_id.clone();
    let attempt = c
        .ledger(id)?
        .read()
        .map_err(crate::error::to_io)?
        .iter()
        .filter(|event| event.kind == "node_started" && event.node_id == node)
        .map(|event| event.attempt)
        .max()
        .unwrap_or(0)
        + 1;
    if control.canceled.load(Ordering::Acquire) {
        return Ok(Outcome::Succeeded(Vec::new()));
    }
    let result = crate::run_job::execute_formation(
        c,
        id,
        mission,
        req,
        &workspace,
        control,
        node.clone(),
        attempt,
        Vec::new(),
        input,
        &inputs,
        "initial",
        false,
        None,
        Vec::new(),
    )?;
    let text = result.text;
    if let Some(escalation) = crate::parse_escalation(&text, id) {
        let node_id = if escalation.node_id.is_empty() {
            node.clone()
        } else {
            escalation.node_id
        };
        let mut raised = event(
            "escalation_raised",
            &node_id,
            attempt,
            json!({"severity":escalation.severity,"reason":escalation.reason,"source":"agent","trigger":"sentinel","blocks":escalation.blocks}),
        );
        raised.gate_id = escalation.gate_id;
        append_live(c, id, control, raised)?;
        if escalation.blocks {
            return Ok(Outcome::Escalated(node_id));
        }
    }
    crate::run_capture::publish(
        c,
        id,
        mission,
        req,
        &workspace,
        control,
        event(
            "node_output",
            &node,
            attempt,
            json!({"status":result.status,"text":text,"outputs":result.outputs,"reportRef":format!("lab://{id}/{node}/report.md")}),
        ),
    )?;
    Ok(Outcome::Succeeded(Vec::new()))
}

#[cfg(test)]
mod output_tests {
    #[test]
    fn picked_run_without_declared_outputs_is_not_successful() {
        let event = super::success_event(
            vec!["done".into()],
            crate::outputs::Resolved {
                outputs: Vec::new(),
                picks: vec![serde_json::json!({"seq":3})],
            },
        );
        assert_eq!(event.kind, "run_picked");
        assert_eq!(event.data["picks"].as_array().unwrap().len(), 1);
    }
}
