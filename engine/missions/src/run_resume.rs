//! Resume a blocked run from its durable snapshots.
use crate::coordinator::Control;
use crate::{BindingsSnapshot, Coordinator, RunEvent};
use parvi_mission_model::{ResumeRequest, RunProjection, RunStart};
use std::{io, path::PathBuf, sync::Arc};

impl Coordinator {
    pub fn resume(&self, id: &str, request: ResumeRequest) -> Result<RunProjection, crate::Error> {
        (|| -> io::Result<RunProjection> {
        let _admission = self.inner.admission.lock().unwrap();
        if self
            .inner
            .shutting_down
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "coordinator is shutting down",
            ));
        }
        if self.inner.controls.lock().unwrap().contains_key(id) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "run is working",
            ));
        }
        let ledger = self.ledger(id)?;
        let events = ledger.read().map_err(crate::error::to_io)?;
        crate::gate_replay::validate(&events)?;
        crate::human_replay::validate(&events)?;
        crate::code_binding::validate_run(self, id, &events)?;
        let projection = crate::project_run(&events, id).map_err(crate::error::to_io)?;
        if projection.status != "blocked" || !projection.resume_allowed {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "run cannot resume",
            ));
        }
        if request.take_fail && projection.block_code != "invalid_judge_result" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "take-fail requires an invalid judge result",
            ));
        }
        if let Some(refusal) = crate::run_limit::resume_refusal(
            &projection.resume_policy,
            request.grant,
            request.pick_best,
        ) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, refusal));
        }
        let runs = self.inner.root.join("runs");
        let mission = parvi_mission_model::Mission::parse(&std::fs::read_to_string(
            runs.join(format!("{id}.snapshot.toml")),
        )?)
        .map_err(|e| io::Error::other(e.to_string()))?;
        let bindings: BindingsSnapshot = toml::from_str(&std::fs::read_to_string(
            runs.join(format!("{id}.bindings.toml")),
        )?)
        .map_err(io::Error::other)?;
        let actor = if request.actor.is_empty() {
            "human:operator"
        } else {
            &request.actor
        };
        let blocked = events
            .iter()
            .rev()
            .find(|e| e.kind == "run_blocked")
            .expect("blocked projection has a block");
        if request.take_fail
            && !events
                .iter()
                .any(|e| e.kind == "gate_evaluating" && e.gate_id == blocked.gate_id)
        {
            return Err(io::Error::other("missing gate evaluation"));
        }
        // One more allowance of the knob the block spent, as the ledger
        // records who gave it: one round, or the card's time or tokens again.
        let grant = if request.grant {
            let refused = || {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "this block is not a spent limit, so there is nothing to grant: resume without --grant",
                )
            };
            let use_ = crate::limits::LimitUse::recorded(blocked).ok_or_else(refused)?;
            let amount = crate::limits::grant_amount(&mission, &use_).ok_or_else(refused)?;
            Some(serde_json::json!({"limitId":use_.limit_id,"kind":use_.kind,"amount":amount}))
        } else {
            None
        };
        let pending = blocked
            .data
            .get("pendingOutput")
            .filter(|value| !value.is_null());
        let redispatch_node = pending
            .and_then(|p| p.get("nodeId"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let redispatch_attempt = pending
            .and_then(|p| p.get("attempt"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let mut data = serde_json::json!({"judgeVerdict":if request.take_fail {"fail"} else {""},"blockedGateId":blocked.gate_id,"resumeMode":if request.mode == parvi_mission_model::ResumeMode::Redispatch {"redispatch"} else {"reattach"},"redispatchNodeId":if request.mode == parvi_mission_model::ResumeMode::Redispatch {redispatch_node} else {""},"redispatchAttempt":if request.mode == parvi_mission_model::ResumeMode::Redispatch {redispatch_attempt} else {0},"resumedBy":actor,"resumedFromSeq":blocked.seq}).as_object().unwrap().clone();
        if let Some(grant) = grant {
            data.insert("grant".into(), grant);
        }
        if request.pick_best {
            // The resumed run picks before anything else (run_pick::on_resume).
            data.insert("pickBest".into(), true.into());
        }
        if !request.reason.trim().is_empty() {
            data.insert("reason".into(), request.reason.trim().into());
        }
        self.append(
            id,
            RunEvent {
                kind: "run_resumed".into(),
                actor: actor.into(),
                timestamp: crate::run_storage::now(),
                data,
                ..Default::default()
            },
        )?;
        let start = RunStart {
            mission: projection.mission_slug,
            input_card_id: projection.input_card_id,
            formation_id: events[0]
                .data
                .get("formationId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into(),
            expected_rev: projection.mission_rev,
            cwd: projection.cwd.clone(),
            repository: projection.repository.clone(),
            base: projection.base.clone(),
            context_paths: projection.context_paths,
            work: projection.work,
            actor: projection.started_by,
            inputs: projection
                .inputs
                .into_iter()
                .map(|i| (i.name, i.value))
                .collect(),
            ..Default::default()
        };
        let control = Arc::new(Control::default());
        let workspace = PathBuf::from(&start.cwd);
        let blocked = blocked.clone();
        self.inner
            .controls
            .lock()
            .unwrap()
            .insert(id.into(), control.clone());
        let owner = self.clone();
        let run_id = id.to_owned();
        std::thread::spawn(move || {
            let resolved = crate::run_reattach::resolve(
                &owner,
                &run_id,
                &mission,
                &start,
                &workspace,
                &control,
                crate::run_reattach::ResumeDispatches {
                    blocked: &blocked,
                    mode: request.mode,
                },
            );
            if control.stopping.load(std::sync::atomic::Ordering::Acquire)
                || control.canceled.load(std::sync::atomic::Ordering::Acquire)
                || matches!(resolved, Ok(true))
            {
                crate::run_worker::execute(
                    owner, run_id, mission, bindings, start, workspace, control,
                );
                return;
            }
            if let Err(error) = resolved {
                let events = owner
                    .ledger(&run_id)
                    .and_then(|ledger| ledger.read().map_err(crate::error::to_io))
                    .unwrap_or_default();
                let open = crate::run_open_dispatch::unresolved(&events);
                let pending = crate::run_recovery::pending_capture(&events);
                let code = crate::run_recovery::publication_error_code(&error.to_string());
                let _ = owner.append(&run_id, RunEvent { kind:"run_blocked".into(), timestamp:crate::run_storage::now(), data:serde_json::json!({"code":code,"reason":error.to_string(),"resumeAllowed":true,"resumePolicy":"explicit","openDispatches":open.iter().map(|e| &e.data).collect::<Vec<_>>(),"pendingOutput":pending}).as_object().unwrap().clone(), ..Default::default() });
            }
            let mut done = control.done.lock().unwrap();
            *done = true;
            control.finished.notify_all();
            drop(done);
            owner.inner.controls.lock().unwrap().remove(&run_id);
        });
        self.run(id).map_err(crate::error::to_io)
     })().map_err(crate::error::io)
    }
}
