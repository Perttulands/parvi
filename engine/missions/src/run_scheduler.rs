//! Schedules runnable work and routes results in ledger order.
use crate::coordinator::Control;
use crate::run_gate::{GateSettlement, settle_gate};
use crate::run_worker::{Outcome, event};
use crate::{Coordinator, Mission, RunStart, render_run_inputs, resolve_run_inputs};
use serde_json::json;
use std::{
    io,
    path::PathBuf,
    sync::{Arc, atomic::Ordering, mpsc},
};

pub(crate) enum Finished {
    Step(String, i64, io::Result<crate::FormationResult>),
    Judge(
        Box<crate::Gate>,
        i64,
        Box<crate::RoutedInput>,
        io::Result<crate::JudgeVerdict>,
    ),
    /// A gate's judge chain finished picking a step's best attempt.
    Pick(String, io::Result<()>),
}

/// Why the run stops once its in-flight work settles.
enum Stop {
    Error(io::Error),
    Escalation(String),
    Limit(crate::run_limit::LimitStop),
}

/// Re-read durable deliveries before deciding what can run next.
pub(crate) fn schedule(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    req: &RunStart,
    workspace: PathBuf,
    control: &Arc<Control>,
) -> io::Result<Outcome> {
    use std::collections::HashSet;
    let inputs = resolve_run_inputs(mission, &req.inputs);
    let input = render_run_inputs(mission, &inputs);
    let mut noted = c.ledger(id)?.read()?.len() as u64;
    if !c
        .ledger(id)?
        .read()?
        .iter()
        .any(|e| e.kind == "node_output" && e.node_id == req.input_card_id)
    {
        append(
            c,
            id,
            control,
            event(
                "node_started",
                &req.input_card_id,
                0,
                json!({"nodeKind":"inputCard","inputRefs":[],"reason":"initial"}),
            ),
        )?;
        append(
            c,
            id,
            control,
            event(
                "node_output",
                &req.input_card_id,
                0,
                json!({"status":"done","text":input,"outputs":{"out":{"text":input}}}),
            ),
        )?;
    }
    let (wake_tx, wake_rx) = mpsc::channel();
    *control.wake.lock().unwrap() = Some(wake_tx.clone());
    let (done_tx, done_rx) = mpsc::channel::<Finished>();
    let mut running = HashSet::new();
    let events = c.ledger(id)?.read()?;
    // Steps run at once and the mission's tokens card covers them all.
    *control.tokens.lock().unwrap() = crate::limits::mission_tokens(mission, &events)
        .map(|use_| Arc::new(crate::attempt_limits::TokenPool::new(use_.max, use_.used)));
    let stop_event = events
        .iter()
        .rev()
        .take_while(|e| e.kind != "run_resumed" && e.kind != "run_started")
        .filter(|e| match e.kind.as_str() {
            // A limit that started a pick stopped nothing.
            "error" => !events.iter().any(|pick| {
                pick.kind == crate::run_pick::PICK_STARTED
                    && pick.seq > e.seq
                    && pick.node_id == e.node_id
            }),
            "escalation_raised" => e.data.get("blocks").and_then(|v| v.as_bool()) == Some(true),
            "judge_attempt_failed" => {
                e.data.get("correction").and_then(|v| v.as_bool()) != Some(true)
            }
            _ => false,
        })
        .last();
    let mut stop = stop_event.map(|e| match e.kind.as_str() {
        "escalation_raised" => Stop::Escalation(e.node_id.clone()),
        "error" if let Some(use_) = crate::limits::LimitUse::recorded(e) => {
            Stop::Limit(crate::run_limit::LimitStop {
                node: e.node_id.clone(),
                use_,
                fail: false,
            })
        }
        "judge_attempt_failed" => Stop::Error(io::Error::other(format!(
            "{}[{}]: {}",
            e.data
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("invalid_judge_result"),
            e.gate_id,
            e.data
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("invalid verdict")
        ))),
        _ => Stop::Error(io::Error::other(
            e.data
                .get("detail")
                .or_else(|| e.data.get("reason"))
                .and_then(|v| v.as_str())
                .unwrap_or("executor_failed")
                .to_owned(),
        )),
    });
    // A restarted worker may finish work already dispatched before the stop,
    // but must never retry the failed node or dispatch a fresh route.
    let mut drain = HashSet::new();
    let mut drain_gates = HashSet::new();
    if let Some(failure) = stop_event {
        let replay = crate::run_replay::Replay::read(mission, &events);
        for (gate, started) in replay.evaluating {
            if started.seq < failure.seq && gate != failure.gate_id && gate != failure.node_id {
                drain_gates.insert(gate);
            }
        }
        for (node, started) in replay.open {
            if started.seq < failure.seq
                && node != failure.node_id
                && failure.data.get("sourceNodeId").and_then(|v| v.as_str()) != Some(node.as_str())
            {
                drain.insert(node);
            }
        }
    }
    let cut = control.limit.lock().unwrap().take();
    if let Some(limit) = cut
        && (stop.is_none() || limit.fail)
    {
        stop = Some(Stop::Limit(limit));
    }
    if stop.is_none()
        && let Some(limit) = crate::run_pick::on_resume(c, id, control, mission)?
    {
        stop = Some(Stop::Limit(limit));
    }
    loop {
        if let Ok(finished) = done_rx.try_recv() {
            match finished {
                Finished::Pick(gate, result) => {
                    running.remove(&format!("gate:{gate}"));
                    if let Err(e) = result
                        && !control.canceled.load(Ordering::Acquire)
                    {
                        failed(c, id, control, mission, &gate, e, &mut stop)?;
                    }
                }
                Finished::Judge(gate, attempt, input, result) => {
                    running.remove(&format!("gate:{}", gate.id));
                    drain_gates.remove(&gate.id);
                    match result {
                        Ok(result) if !control.canceled.load(Ordering::Acquire) => {
                            let evidence: Vec<_> = result
                                .evidence
                                .iter()
                                .map(|text| json!({"kind":"formation","text":text}))
                                .collect();
                            let mut kind = event(
                                "gate_kind_result",
                                &gate.id,
                                0,
                                json!({"kind":"formation","verdict":result.verdict,"reason":result.reason,"evidence":evidence,"inputRef":input}),
                            );
                            kind.gate_id = gate.id.clone();
                            append(c, id, control, kind)?;
                            settle_gate(
                                c,
                                id,
                                control,
                                mission,
                                GateSettlement {
                                    gate: &gate,
                                    attempt,
                                    input: &input,
                                    result: &result,
                                },
                            )?;
                        }
                        Ok(_) => {}
                        Err(e) if !control.canceled.load(Ordering::Acquire) => {
                            failed(c, id, control, mission, &gate.id, e, &mut stop)?;
                        }
                        Err(_) => {}
                    }
                }
                Finished::Step(node, attempt, result) => {
                    running.remove(&node);
                    match result {
                        Ok(result) if !control.canceled.load(Ordering::Acquire) => {
                            let mut blocks = false;
                            if let Some(escalation) = crate::parse_escalation(&result.text, id) {
                                let target = if escalation.node_id.is_empty() {
                                    node.clone()
                                } else {
                                    escalation.node_id
                                };
                                let mut raised = event(
                                    "escalation_raised",
                                    &target,
                                    attempt,
                                    json!({"severity":escalation.severity,"reason":escalation.reason,"source":"agent","sourceNodeId":node,"trigger":"sentinel","blocks":escalation.blocks}),
                                );
                                raised.gate_id = escalation.gate_id;
                                append(c, id, control, raised)?;
                                if escalation.blocks {
                                    blocks = true;
                                    if stop.is_none() {
                                        stop = Some(Stop::Escalation(target));
                                    }
                                }
                            }
                            if !blocks {
                                let output = event(
                                    "node_output",
                                    &node,
                                    attempt,
                                    json!({"status":result.status,"text":result.text,"outputs":result.outputs,"reportRef":format!("lab://{id}/{node}/report.md")}),
                                );
                                if let Err(error) = crate::run_capture::publish(
                                    c, id, mission, req, &workspace, control, output,
                                ) && !control.canceled.load(Ordering::Acquire)
                                {
                                    record_error(c, id, control, &node, &error)?;
                                    if stop.is_none() {
                                        stop = Some(Stop::Error(error));
                                    }
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(e) if !control.canceled.load(Ordering::Acquire) => {
                            failed(c, id, control, mission, &node, e, &mut stop)?;
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        if control.canceled.load(Ordering::Acquire) && running.is_empty() {
            return Ok(Outcome::Succeeded(Vec::new()));
        }
        if control.canceled.load(Ordering::Acquire) {
            wake_rx.recv().map_err(io::Error::other)?;
            continue;
        }
        if control.stopping.load(Ordering::Acquire) {
            if running.is_empty() {
                return Ok(Outcome::Stopped);
            }
            wake_rx.recv().map_err(io::Error::other)?;
            continue;
        }
        let events = c.ledger(id)?.read()?;
        if let Err(error) = crate::gate_replay::validate(&events)
            .and_then(|()| crate::human_replay::validate(&events))
            .and_then(|()| crate::code_binding::validate_run(c, id, &events))
        {
            if running.is_empty() {
                return Err(error);
            }
            record_error(c, id, control, "", &error)?;
            if stop.is_none() {
                stop = Some(Stop::Error(error));
            }
            wake_rx.recv().map_err(io::Error::other)?;
            continue;
        }
        let replay = crate::run_replay::Replay::read(mission, &events);
        let previous_noted = noted;
        noted = events.len() as u64;
        if crate::run_replay::note_waiting(
            c,
            id,
            control,
            mission,
            &replay,
            (previous_noted, noted),
        )? {
            continue;
        }
        if crate::human_replay::route(c, id, control, mission, &events)? {
            continue;
        }
        if !control.canceled.load(Ordering::Acquire) {
            let gate_context = crate::run_gate::GateContext {
                c,
                id,
                control,
                mission,
                req,
                workspace: &workspace,
                inputs: &inputs,
                events: &events,
                replay: &replay,
                done: &done_tx,
                wake: &wake_tx,
                drain_gates: &drain_gates,
            };
            if stop.is_none() {
                match crate::run_pick::advance(&gate_context, &running)? {
                    crate::run_pick::PickAction::Acted => continue,
                    crate::run_pick::PickAction::Judging(gate) => {
                        running.insert(format!("gate:{gate}"));
                        continue;
                    }
                    crate::run_pick::PickAction::None => {}
                }
            }
            match crate::run_gate::advance(gate_context, &mut running, stop.is_some())? {
                crate::run_gate::GateAction::Acted => continue,
                crate::run_gate::GateAction::Failed(node, error) => {
                    drain_gates.remove(&node);
                    record_error(c, id, control, &node, &error)?;
                    if stop.is_none() {
                        stop = Some(Stop::Error(error));
                    }
                    continue;
                }
                crate::run_gate::GateAction::None => {}
            }
            if (stop.is_none() || !drain.is_empty())
                && crate::run_job::start_ready(
                    crate::run_job::JobContext {
                        c,
                        id,
                        control,
                        mission,
                        req,
                        workspace: &workspace,
                        inputs: &inputs,
                        events: &events,
                        replay: &replay,
                        done: &done_tx,
                        wake: &wake_tx,
                        drain: stop.as_ref().map(|_| &mut drain),
                    },
                    &mut running,
                )?
            {
                continue;
            }
        }
        if running.is_empty() {
            if let Some(stop) = stop {
                return match stop {
                    Stop::Error(error) => Err(error),
                    Stop::Escalation(node) => Ok(Outcome::Escalated(node)),
                    Stop::Limit(limit) => Ok(Outcome::Limit(limit)),
                };
            }
            let projection = crate::RunProjection::from_events(&events, id)?;
            if !replay.unfinished.is_empty() {
                return Ok(Outcome::Unfinished(replay.unfinished.into_iter().collect()));
            }
            if projection.waiting_gates.is_empty() {
                let mut end_ids = Vec::new();
                for (end, _, _, _, _) in &replay.ended {
                    if !end_ids.contains(end) {
                        end_ids.push(end.clone());
                    }
                }
                if let Some((end, _, gate, reason, actor)) = replay
                    .ended
                    .iter()
                    .find(|(_, outcome, _, _, _)| outcome == "rejected")
                {
                    return Ok(Outcome::Rejected {
                        end_id: end.clone(),
                        gate_id: gate.clone(),
                        reason: reason.clone(),
                        actor: actor.clone(),
                        end_ids,
                    });
                }
                let starved: Vec<_> = mission
                    .formations
                    .iter()
                    .filter_map(|formation| {
                        let ports = replay.ports.get(&formation.id)?;
                        if !ports.values().any(|(_, unread)| *unread) {
                            return None;
                        }
                        let missing: Vec<_> = formation
                            .inputs
                            .iter()
                            .filter(|port| !ports.contains_key(&port.id))
                            .map(|port| port.id.clone())
                            .collect();
                        (!missing.is_empty())
                            .then(|| (formation.id.clone(), formation.title.clone(), missing))
                    })
                    .collect();
                if !starved.is_empty() {
                    return Ok(Outcome::Starved(starved));
                }
                if replay.ended.is_empty() {
                    return Ok(Outcome::Unfinished(vec![req.input_card_id.clone()]));
                }
                return Ok(Outcome::Succeeded(end_ids));
            }
        }
        wake_rx.recv().map_err(io::Error::other)?;
    }
}

fn append(c: &Coordinator, id: &str, control: &Control, event: crate::RunEvent) -> io::Result<()> {
    crate::run_worker::append_live(c, id, control, event).map(|_| ())
}

/// What a failed step, judge chain or pick does to the run: a spent limit
/// picks or stops the run at it, anything else records the error and stops
/// the run. A card whose whenSpent is fail outranks any other stop.
fn failed(
    c: &Coordinator,
    id: &str,
    control: &Control,
    mission: &Mission,
    node: &str,
    error: io::Error,
    stop: &mut Option<Stop>,
) -> io::Result<()> {
    if let Some(reached) = crate::run_limit::of(&error) {
        if let Some(limit) = crate::run_limit::pick_or_stop(c, id, control, mission, reached)? {
            let outranks = match stop {
                None => true,
                Some(Stop::Limit(old)) => limit.fail && !old.fail,
                Some(_) => limit.fail,
            };
            if outranks {
                *stop = Some(Stop::Limit(limit));
            }
        }
        return Ok(());
    }
    record_error(c, id, control, node, &error)?;
    if stop.is_none() {
        *stop = Some(Stop::Error(error));
    }
    Ok(())
}

fn record_error(
    c: &Coordinator,
    id: &str,
    control: &Control,
    node: &str,
    error: &io::Error,
) -> io::Result<()> {
    let detail = error.to_string();
    let code = crate::run_worker::error_code(&detail);
    if code == "invalid_judge_result" || code == crate::run_pick::INVALID_JUDGE_PICK {
        let events = c.ledger(id)?.read()?;
        let terminal = events
            .iter()
            .rev()
            .take_while(|e| {
                (e.kind != "gate_evaluating" && e.kind != crate::run_pick::PICK_STARTED)
                    || e.gate_id != node
            })
            .any(|e| {
                e.kind == "judge_attempt_failed"
                    && e.gate_id == node
                    && e.data.get("code").and_then(|v| v.as_str()) == Some(code)
                    && e.data.get("correction").and_then(|v| v.as_bool()) != Some(true)
            });
        if terminal {
            return Ok(());
        }
    }
    append(
        c,
        id,
        control,
        event(
            "error",
            node,
            0,
            json!({"code":code,"detail":detail,"message":detail,"reason":detail,"nodeId":node,"recoverable":true}),
        ),
    )
}
