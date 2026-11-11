//! Resolve dispatches named by a block before scheduling new work.
use crate::coordinator::Control;
use crate::{Coordinator, Mission, OpenDispatch, ResumeMode, RunEvent, RunStart};
use serde_json::json;
use std::{io, path::Path};

fn request(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    dispatch: &RunEvent,
    control: Option<&Control>,
) -> Result<OpenDispatch, String> {
    let formation = mission
        .formation(&dispatch.node_id)
        .cloned()
        .ok_or_else(|| "formation missing from snapshot".to_owned())?;
    Ok(OpenDispatch {
        run_id: id.into(),
        dispatch_id: dispatch
            .data
            .get("dispatchId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        node_id: dispatch.node_id.clone(),
        slot_id: dispatch.slot_id.clone(),
        attempt: dispatch.attempt,
        formation,
        dispatch_data: dispatch.data.clone(),
        artifact_dir: crate::run_capture::attempt_dir(c, id, &dispatch.node_id, dispatch.attempt)
            .map_err(|e| e.to_string())?,
        canceled: control.map(|control| control.canceled.clone()),
        stopping: control.map(|control| control.stopping.clone()),
        final_judge: mission.gates.iter().any(|gate| {
            crate::graph::judge_chain(mission, &gate.id)
                .last()
                .is_some_and(|judge| judge.id == dispatch.node_id)
        }),
        limits: Default::default(),
    })
}

/// Stage completed evidence from a blocked run without changing its ledger.
pub(crate) fn stage_completed(c: &Coordinator, id: &str, mission: &Mission, dispatch: &RunEvent) {
    if let Ok(request) = request(c, id, mission, dispatch, None)
        && let Some(result) = c.inner.executor.recover_completed(request.clone())
        && valid(&request.formation, &result)
    {
        c.inner
            .recovered
            .lock()
            .unwrap()
            .insert((id.into(), request.dispatch_id), result);
    }
}

pub(crate) fn recover_completed(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
    dispatch: &RunEvent,
) -> io::Result<bool> {
    let Ok(request) = request(c, id, mission, dispatch, Some(control)) else {
        return Ok(false);
    };
    let Some(result) = c.inner.executor.recover_completed(request.clone()) else {
        return Ok(false);
    };
    if !valid(&request.formation, &result) {
        return Ok(false);
    }
    keep(c, id, mission, req, workspace, control, dispatch, result)?;
    Ok(true)
}

fn valid(formation: &crate::Formation, result: &crate::FormationResult) -> bool {
    result.status == "done"
        && formation
            .outputs
            .iter()
            .all(|p| result.outputs.contains_key(&p.id))
        && result
            .outputs
            .keys()
            .all(|id| formation.outputs.iter().any(|p| &p.id == id))
}

#[allow(clippy::too_many_arguments)]
fn keep(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
    dispatch: &RunEvent,
    result: crate::FormationResult,
) -> io::Result<()> {
    let dispatch_id = dispatch
        .data
        .get("dispatchId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let request = request(c, id, mission, dispatch, Some(control)).map_err(io::Error::other)?;
    let prior = c.ledger(id)?.read().map_err(crate::error::to_io)?;
    for replay in c
        .inner
        .executor
        .completed_events(&request)
        .map_err(io::Error::other)?
    {
        let already_recorded = prior.iter().any(|event| {
            event.node_id == dispatch.node_id
                && event.attempt == dispatch.attempt
                && event.kind == replay.kind
                && event.data.get("dispatchId") == replay.data.get("dispatchId")
                && (replay.kind == "slot_dispatch"
                    || replay.kind == "workspace_turn"
                    || event.data.get("seatEventSeq").is_some()
                        && event.data.get("seatEventSeq") == replay.data.get("seatEventSeq"))
        });
        if already_recorded {
            continue;
        }
        let mut record = crate::run_worker::event(
            &replay.kind,
            &dispatch.node_id,
            dispatch.attempt,
            serde_json::Value::Object(replay.data),
        );
        record.slot_id = replay.slot_id;
        c.append(id, record)?;
    }
    let mut slot = crate::run_worker::event(
        "slot_result",
        &dispatch.node_id,
        dispatch.attempt,
        json!({"dispatchId":dispatch_id,"status":"ok","sentinel":{"runId":id,"status":"ok"}}),
    );
    slot.slot_id = dispatch.slot_id.clone();
    if !c
        .ledger(id)?
        .read()
        .map_err(crate::error::to_io)?
        .iter()
        .any(|e| {
            e.kind == "slot_result"
                && e.data.get("dispatchId").and_then(|v| v.as_str()) == Some(dispatch_id)
        })
    {
        c.append(id, slot)?;
    }
    let events = c.ledger(id)?.read().map_err(crate::error::to_io)?;
    let reason = events
        .iter()
        .rev()
        .find(|e| {
            e.kind == "node_started"
                && e.node_id == dispatch.node_id
                && e.attempt == dispatch.attempt
        })
        .and_then(|e| e.data.get("reason"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut output = crate::run_worker::event(
        "node_output",
        &dispatch.node_id,
        dispatch.attempt,
        json!({"status":result.status,"text":result.text,"outputs":result.outputs,"reportRef":format!("lab://{id}/{}/report.md",dispatch.node_id)}),
    );
    if reason == "judge" {
        output.data.insert("reason".into(), "judge".into());
    }
    if reason == "judge" {
        crate::run_workspace::publish_output(c, id, req, workspace, control, output)
    } else {
        crate::run_capture::publish(c, id, mission, req, workspace, control, output)
    }
}

/// Record what a reattached dispatch's seats spent, as a live one records
/// it when it ends, so later attempts and the mission's card count it.
fn record_tokens(
    c: &Coordinator,
    id: &str,
    control: &Control,
    dispatch: &RunEvent,
    limits: &crate::AttemptLimits,
) -> io::Result<()> {
    for (dispatch_id, tokens) in limits.counted_tokens() {
        let mut usage = crate::run_worker::event(
            "token_usage",
            &dispatch.node_id,
            dispatch.attempt,
            json!({"dispatchId":dispatch_id,"tokens":tokens}),
        );
        usage.slot_id.clone_from(&dispatch.slot_id);
        crate::run_worker::append_live(c, id, control, usage)?;
    }
    Ok(())
}

/// A limit ran out while a reattached dispatch worked: its attempt is cut
/// off as a live one is, and the run picks or keeps the stop for its worker.
fn cut_off(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    control: &Control,
    dispatch: &RunEvent,
    limits: &crate::AttemptLimits,
) -> io::Result<()> {
    let error = crate::run_job::cut_off(
        c,
        id,
        mission,
        control,
        &dispatch.node_id,
        dispatch.attempt,
        limits,
    )?;
    let Some(reached) = crate::run_limit::of(&error) else {
        return Err(error);
    };
    if let Some(stop) = crate::run_limit::pick_or_stop(c, id, control, mission, reached)? {
        let mut held = control.limit.lock().unwrap();
        if held.as_ref().is_none_or(|old| stop.fail && !old.fail) {
            *held = Some(stop);
        }
    }
    Ok(())
}

pub(crate) struct ResumeDispatches<'a> {
    pub blocked: &'a RunEvent,
    pub mode: ResumeMode,
}

pub(crate) fn resolve(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
    resume: ResumeDispatches<'_>,
) -> io::Result<bool> {
    let ResumeDispatches { blocked, mode } = resume;
    let events = c.ledger(id)?.read().map_err(crate::error::to_io)?;
    let named: Vec<_> = blocked
        .data
        .get("openDispatches")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.get("dispatchId").and_then(|v| v.as_str()))
        .collect();
    let mut failed = Vec::new();
    let mut reasons = Vec::new();
    for dispatch in crate::run_open_dispatch::unresolved(&events) {
        if control.stopping.load(std::sync::atomic::Ordering::Acquire)
            || control.canceled.load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(true);
        }
        // Keeping a corrected answer also resolves its correction dispatch.
        if !crate::run_open_dispatch::unresolved(
            &c.ledger(id)?.read().map_err(crate::error::to_io)?,
        )
        .iter()
        .any(|open| open.seq == dispatch.seq)
        {
            continue;
        }
        let dispatch_id = dispatch
            .data
            .get("dispatchId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if mode == ResumeMode::Redispatch || !named.contains(&dispatch_id) {
            c.inner
                .recovered
                .lock()
                .unwrap()
                .remove(&(id.to_owned(), dispatch_id.to_owned()));
            let mut abandoned = crate::run_worker::event(
                "slot_result",
                &dispatch.node_id,
                dispatch.attempt,
                json!({"dispatchId":dispatch_id,"slotId":dispatch.slot_id,"status":"abandoned"}),
            );
            abandoned.slot_id = dispatch.slot_id.clone();
            c.append(id, abandoned)?;
            continue;
        }
        // A dispatch still working is held to its step's Limit cards, which
        // count what it worked before the restart.
        let mut limits = None;
        let result = request(c, id, mission, &dispatch, Some(control)).and_then(|mut request| {
            let staged = c
                .inner
                .recovered
                .lock()
                .unwrap()
                .remove(&(id.to_owned(), request.dispatch_id.clone()));
            let result = match staged
                .or_else(|| c.inner.executor.recover_completed(request.clone()))
            {
                Some(result) => result,
                None => {
                    let (held, _timer) =
                        crate::run_job::attempt_limits(c, id, mission, control, &dispatch.node_id)
                            .map_err(|e| e.to_string())?;
                    request.limits = held.clone();
                    limits = Some(held);
                    c.inner
                        .executor
                        .reattach(request.clone())
                        .map_err(|error| error.message)?
                }
            };
            if !valid(&request.formation, &result) {
                return Err("reattached formation returned invalid outputs".into());
            }
            Ok(result)
        });
        if control.stopping.load(std::sync::atomic::Ordering::Acquire)
            || control.canceled.load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(true);
        }
        if let Some(limits) = &limits {
            record_tokens(c, id, control, &dispatch, limits)?;
            if limits.is_spent() {
                cut_off(c, id, mission, control, &dispatch, limits)?;
                continue;
            }
        }
        match result {
            Ok(result) => {
                keep(c, id, mission, req, workspace, control, &dispatch, result)?;
            }
            Err(reason) => {
                let mut error = crate::run_worker::event(
                    "error",
                    &dispatch.node_id,
                    dispatch.attempt,
                    json!({"code":"dispatch_reattach_failed","reason":reason,"dispatchId":dispatch_id,"recoverable":true}),
                );
                error.slot_id = dispatch.slot_id.clone();
                c.append(id, error)?;
                reasons.push(format!("{}: {reason}", dispatch.node_id));
                failed.push(dispatch.data.clone());
            }
        }
    }
    if !failed.is_empty() {
        c.append(id, RunEvent { kind:"run_blocked".into(), timestamp:crate::run_storage::now(), data:json!({"code":"dispatch_reattach_failed","reason":format!("dispatch reattach failed: {}", reasons.join("; ")),"resumeAllowed":true,"resumePolicy":"explicit","openDispatches":failed}).as_object().unwrap().clone(), ..Default::default() })?;
        return Ok(false);
    }
    Ok(true)
}
