//! Executes one formation attempt and validates its dispatches.
use crate::coordinator::Control;
use crate::run_worker::event;
use crate::{
    BindingsSnapshot, Coordinator, FormationExecution, Mission, RunStart, mission_run_inputs,
    substitute_run_inputs,
};
use serde_json::json;
use std::{
    io,
    path::PathBuf,
    sync::{atomic::Ordering, mpsc},
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_formation(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    req: &RunStart,
    workspace: &std::path::Path,
    control: &Control,
    node: String,
    attempt: i64,
    refs: Vec<serde_json::Value>,
    text: String,
    inputs: &[crate::RunInput],
    reason: &str,
    final_judge: bool,
    started: Option<mpsc::SyncSender<()>>,
    pick_attempts: Vec<i64>,
) -> io::Result<crate::FormationResult> {
    let mut formation = mission
        .formation(&node)
        .cloned()
        .ok_or_else(|| io::Error::other("route requires the gate scheduler"))?;
    let source = std::fs::read_to_string(
        c.inner
            .root
            .join("runs")
            .join(format!("{id}.bindings.toml")),
    )?;
    let bindings: BindingsSnapshot =
        toml::from_str(&source).map_err(|_| io::Error::other("persona_snapshot_invalid"))?;
    for slot in &formation.slots {
        bindings
            .binding(mission, id, &req.input_card_id, &node, slot)
            .map_err(io::Error::other)?;
    }
    if control.stopping.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "coordinator is shutting down",
        ));
    }
    // The run's Limit cards are read and the start recorded under one lock,
    // so two steps starting at once never share a card's last round.
    let starts = control.starts.lock().unwrap();
    let events = c.ledger(id)?.read().map_err(crate::error::to_io)?;
    if let Some(spent) =
        crate::limits::spent_before(mission, &events, &node, time::OffsetDateTime::now_utc())
    {
        return Err(crate::run_limit::reached(&node, spent, false));
    }
    crate::run_left_seats::end(c, id, Some(&node), "new_attempt")?;
    if let Some(brief) = formation.brief.as_mut() {
        brief.goal = substitute_run_inputs(
            &brief.goal,
            &mission_run_inputs(mission),
            inputs,
            &crate::run_inputs::work_placeholder(&req.work),
        );
    }
    if !crate::run_worker::append_live(
        c,
        id,
        control,
        event(
            "node_started",
            &node,
            attempt,
            json!({"nodeKind":"formation","reason":reason,"inputRefs":refs,"brief":formation.brief}),
        ),
    )? {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
    }
    drop(starts);
    if let Some(started) = started {
        let _ = started.send(());
    }
    let (limits, _timer) = attempt_limits(c, id, mission, control, &node)?;
    let placed = !formation.device.is_empty() && formation.device != c.inner.settings.environment;
    let (turn_workspace, turn) = if placed {
        let turn = crate::run_workspace::prepare_placed_turn(
            c,
            id,
            req,
            workspace,
            control,
            (node.as_str(), formation.device.as_str()),
            attempt,
        )?;
        (workspace.to_path_buf(), Some(turn))
    } else {
        let path =
            crate::run_workspace::prepare_turn(c, id, req, workspace, control, &node, attempt)?;
        (path, None)
    };
    let call = FormationExecution {
        run_id: id.into(),
        formation: formation.clone(),
        bindings,
        cwd: turn_workspace,
        turn,
        artifact_dir: crate::run_capture::attempt_dir(c, id, &node, attempt)?,
        context_paths: req.context_paths.iter().map(PathBuf::from).collect(),
        work: req.work.clone(),
        inputs: inputs.to_vec(),
        input_text: text.clone(),
        attempt,
        judge: final_judge,
        pick_attempts,
        canceled: control.canceled.clone(),
        stopping: control.stopping.clone(),
        limits: limits.clone(),
    };
    std::fs::create_dir_all(&call.artifact_dir)?;
    let (tx, rx) = mpsc::channel();
    let executor = c.inner.executor.clone();
    let thread = std::thread::spawn(move || {
        executor.execute_formation(call, crate::ExecutionSink { sender: tx })
    });
    let mut write_error = None;
    let mut dispatches = std::collections::BTreeMap::new();
    let mut completed = std::collections::HashSet::new();
    for pending in rx {
        let evt = pending.event;
        if evt.kind == "seat_cleanup"
            && control.stopping.load(Ordering::Acquire)
            && evt.data.get("outcome").and_then(|v| v.as_str()) == Some("left_shutdown")
        {
            let mut record = event(
                &evt.kind,
                &node,
                attempt,
                serde_json::Value::Object(evt.data),
            );
            record.slot_id = evt.slot_id;
            let result = c.append(id, record).map_err(|e| e.to_string());
            let _ = pending.ack.send(result);
            continue;
        }
        if !control.canceled.load(Ordering::Acquire) && write_error.is_none() {
            let dispatch_id = evt
                .data
                .get("dispatchId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let invalid = match evt.kind.as_str() {
                "slot_dispatch" => {
                    if !dispatch_id.is_empty()
                        && !dispatches.contains_key(&dispatch_id)
                        && formation.slots.iter().any(|slot| slot.id == evt.slot_id)
                    {
                        ""
                    } else {
                        "unknown_dispatch"
                    }
                }
                "slot_result" => {
                    if dispatches.get(&dispatch_id) != Some(&evt.slot_id) {
                        "unknown_dispatch"
                    } else if evt
                        .data
                        .get("sentinel")
                        .and_then(|v| v.get("runId"))
                        .and_then(|v| v.as_str())
                        != Some(id)
                    {
                        "completion_sentinel_timeout"
                    } else {
                        ""
                    }
                }
                _ => "",
            };
            if !invalid.is_empty() {
                let err = io::Error::other(invalid);
                let _ = pending.ack.send(Err(err.to_string()));
                write_error = Some(err);
                continue;
            }
            if evt.kind == "limit_warning" {
                let mut warning = event(
                    &evt.kind,
                    &node,
                    attempt,
                    serde_json::Value::Object(evt.data),
                );
                warning.slot_id = evt.slot_id;
                let _ = pending.ack.send(claim_warning(c, id, control, warning));
                continue;
            }
            let kind = evt.kind.clone();
            let slot_id = evt.slot_id.clone();
            let mut e = event(
                &evt.kind,
                &node,
                attempt,
                serde_json::Value::Object(evt.data),
            );
            e.slot_id = evt.slot_id;
            match crate::run_worker::append_live(c, id, control, e) {
                Ok(true) => {
                    if kind == "slot_dispatch" {
                        dispatches.insert(dispatch_id, slot_id);
                    } else if kind == "slot_result" {
                        dispatches.remove(&dispatch_id);
                        completed.insert(slot_id);
                    }
                    let _ = pending.ack.send(Ok(()));
                }
                Ok(false) => {
                    let _ = pending.ack.send(Err("run canceled".into()));
                    write_error = Some(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
                }
                Err(err) => {
                    let _ = pending.ack.send(Err(err.to_string()));
                    write_error = Some(err);
                }
            }
        } else {
            let _ = pending
                .ack
                .send(Err("run canceled or ledger unavailable".into()));
        }
    }
    let result = thread
        .join()
        .map_err(|_| io::Error::other("executor panicked"))?;
    if let Some(e) = write_error {
        return Err(e);
    }
    if control.canceled.load(Ordering::Acquire) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
    }
    if limits.is_spent() {
        return Err(cut_off(c, id, mission, control, &node, attempt, &limits)?);
    }
    let mut result = result.map_err(io::Error::other)?;
    if result.status != "done" {
        return Err(io::Error::other(format!(
            "executor_failed: formation returned {:?}",
            result.status
        )));
    }
    if !dispatches.is_empty() {
        return Err(io::Error::other("formation has unfinished dispatches"));
    }
    if formation
        .slots
        .iter()
        .any(|slot| !completed.contains(&slot.id))
    {
        return Err(io::Error::other("formation has missing slot results"));
    }
    for port in &formation.outputs {
        if !result.outputs.contains_key(&port.id) {
            return Err(io::Error::other(format!(
                "missing_output_payload: formation {} did not produce output for port {}",
                formation.id, port.id
            )));
        }
    }
    if result
        .outputs
        .keys()
        .any(|id| !formation.outputs.iter().any(|port| &port.id == id))
    {
        return Err(io::Error::other(format!(
            "invalid_output_payloads: formation {} produced an unknown output port",
            formation.id
        )));
    }

    crate::output_refs::materialize(&formation, &mut result.outputs)?;
    Ok(result)
}

/// What the Limit cards covering a just-started or reattached attempt allow
/// it, counting the time and tokens the ledger already holds: a timer that
/// raises `spent` when the tightest time card runs out (dropping the returned
/// sender stops it), the warnings its seats get, and the meter its live token
/// counts go to.
pub(crate) fn attempt_limits(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    control: &Control,
    node: &str,
) -> io::Result<(crate::AttemptLimits, Option<mpsc::Sender<()>>)> {
    let events = c.ledger(id)?.read().map_err(crate::error::to_io)?;
    let (time, warnings) =
        crate::limits::time_budget(mission, &events, node, time::OffsetDateTime::now_utc());
    let tokens = crate::limits::token_budget(mission, &events, node).map(|use_| {
        let pool = control.tokens.lock().unwrap().clone();
        std::sync::Arc::new(crate::TokenMeter::new(use_.max - use_.used, pool))
    });
    let limits = crate::AttemptLimits {
        warnings,
        tokens,
        ..Default::default()
    };
    let timer = time.map(|use_| {
        let (stop, stopped) = mpsc::channel::<()>();
        let limits = limits.clone();
        let left = std::time::Duration::from_secs((use_.max - use_.used).max(0) as u64);
        std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(left) {
                limits.time_out();
            }
        });
        stop
    });
    Ok((limits, timer))
}

/// Record a seat's time warning unless the ledger refuses it: each seat of an
/// attempt is warned by a card once, and the mission's card warns only the
/// attempts working when it first fired. A refused warning is not pasted.
fn claim_warning(
    c: &Coordinator,
    id: &str,
    control: &Control,
    warning: crate::RunEvent,
) -> Result<(), String> {
    let _guard = control.done.lock().unwrap();
    if control.canceled.load(Ordering::Acquire) {
        return Err("run canceled".into());
    }
    let appended = c
        .ledger(id)
        .and_then(|ledger| {
            ledger.append_checked(warning.clone(), |events| {
                if crate::limits::warning_refused(events, &warning) {
                    Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "this seat was already warned",
                    ))
                } else {
                    Ok(())
                }
            })
        })
        .map_err(|e| e.to_string())?;
    c.changed(id, appended.seq);
    Ok(())
}

/// The limit that cut a working attempt short: its unresolved dispatches are
/// abandoned, and the use names the card that ran out.
pub(crate) fn cut_off(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    control: &Control,
    node: &str,
    attempt: i64,
    limits: &crate::AttemptLimits,
) -> io::Result<io::Error> {
    crate::run_limit::abandon_cut_off(c, id, control, node, attempt)?;
    let events = c.ledger(id)?.read().map_err(crate::error::to_io)?;
    let now = time::OffsetDateTime::now_utc();
    let use_ = if let Some(meter) = &limits.tokens
        && (meter.pool_spent() || !limits.timed_out())
    {
        let use_ = if meter.pool_spent() {
            crate::limits::mission_tokens(mission, &events)
        } else {
            crate::limits::token_budget(mission, &events, node)
        };
        use_.map(|mut use_| {
            use_.used = use_.used.max(use_.max);
            use_
        })
    } else {
        crate::limits::time_budget(mission, &events, node, now)
            .0
            .map(|mut use_| {
                use_.used = use_.max;
                use_
            })
    };
    let use_ =
        use_.ok_or_else(|| io::Error::other("a limit cut the attempt but no card covers it"))?;
    Ok(crate::run_limit::reached(node, use_, true))
}

/// Inputs and channels used to start the next ready formation.
pub(crate) struct JobContext<'a> {
    pub c: &'a Coordinator,
    pub id: &'a str,
    pub control: &'a std::sync::Arc<Control>,
    pub mission: &'a Mission,
    pub req: &'a RunStart,
    pub workspace: &'a std::path::Path,
    pub inputs: &'a [crate::RunInput],
    pub events: &'a [crate::RunEvent],
    pub replay: &'a crate::run_replay::Replay,
    pub done: &'a mpsc::Sender<crate::run_scheduler::Finished>,
    pub wake: &'a mpsc::Sender<()>,
    pub drain: Option<&'a mut std::collections::HashSet<String>>,
}

pub(crate) fn start_ready(
    context: JobContext<'_>,
    running: &mut std::collections::HashSet<String>,
) -> io::Result<bool> {
    use crate::run_scheduler::Finished;
    let JobContext {
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
        mut drain,
    } = context;
    for formation in &mission.formations {
        if drain.as_ref().is_some_and(|d| !d.contains(&formation.id)) {
            continue;
        }
        if running.contains(&formation.id) {
            continue;
        }
        let refs: Vec<_> = if let Some(open) = replay.open.get(&formation.id) {
            open.data
                .get("inputRefs")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default()
        } else if replay.unfinished.contains(&formation.id)
            && events.iter().any(|event| event.kind == "run_resumed")
        {
            events
                .iter()
                .rev()
                .find(|event| event.kind == "node_started" && event.node_id == formation.id)
                .and_then(|event| event.data.get("inputRefs"))
                .and_then(|value| value.as_array())
                .cloned()
                .unwrap_or_default()
        } else {
            let Some(ports) = replay.ports.get(&formation.id) else {
                continue;
            };
            if !formation.inputs.iter().all(|p| ports.contains_key(&p.id))
                || !ports.values().any(|(_, unread)| *unread)
            {
                continue;
            }
            formation
                .inputs
                .iter()
                .filter_map(|p| ports.get(&p.id))
                .map(|(input, _)| serde_json::to_value(input).unwrap())
                .collect()
        };
        let routed: Vec<crate::RoutedInput> = refs
            .iter()
            .cloned()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()
            .map_err(io::Error::other)?;
        let reason = if routed.iter().any(|input| input.response.is_some()) {
            "continue"
        } else {
            "initial"
        };
        let mut text = if routed
            .iter()
            .any(|input| input.feedback.is_some() || input.response.is_some())
        {
            crate::port_delivery::render_inputs(&routed)
        } else {
            routed
                .iter()
                .map(|input| input.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        };
        for input in routed
            .iter()
            .filter(|input| input.kind == "file" || input.kind == "folder")
        {
            text.push_str(&format!("\ninput path: {}\n", input.r#ref));
        }
        if routed
            .iter()
            .any(|input| input.kind == "file" || input.kind == "folder")
        {
            text.push_str("Input paths are kept copies of earlier work and are read-only; write new versions in your own attempt directory.\n");
        }
        let attempt = replay.attempts.get(&formation.id).copied().unwrap_or(0) + 1;
        let node = formation.id.clone();
        if let Some(drain) = &mut drain {
            drain.remove(&node);
        }
        running.insert(node.clone());
        let (started, start_ack) = mpsc::sync_channel(0);
        let (c, id, mission, req, workspace, control, inputs, done, wake) = (
            c.clone(),
            id.to_owned(),
            mission.clone(),
            req.clone(),
            workspace.to_path_buf(),
            control.clone(),
            inputs.to_vec(),
            done_tx.clone(),
            wake_tx.clone(),
        );
        std::thread::spawn(move || {
            let result = execute_formation(
                &c,
                &id,
                &mission,
                &req,
                &workspace,
                &control,
                node.clone(),
                attempt,
                refs,
                text,
                &inputs,
                reason,
                false,
                Some(started),
                Vec::new(),
            );
            let _ = done.send(Finished::Step(node, attempt, result));
            let _ = wake.send(());
        });
        let _ = start_ack.recv();
        return Ok(true);
    }
    Ok(false)
}
