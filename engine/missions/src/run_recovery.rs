//! Restart interrupted runs from their durable snapshots.
use crate::coordinator::Control;
use crate::{BindingsSnapshot, Coordinator, RunStart};
use std::{io, path::PathBuf, sync::Arc};

impl Coordinator {
    pub(crate) fn recover(&self) -> io::Result<()> {
        let runs = self.inner.root.join("runs");
        let mut interrupted = Vec::new();
        for entry in std::fs::read_dir(&runs)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(id) = name.strip_suffix(".ndjson") else {
                continue;
            };
            if self.inner.unmigrated.contains_key(id) {
                continue;
            }
            let projection = self.run(id).map_err(crate::error::to_io)?;
            if projection.r#final {
                continue;
            }
            if projection.status == "blocked" {
                // A blocked epoch cannot accept completion events. Leave the
                // evidence staged for explicit resume to reconcile.
                if projection.resume_allowed {
                    let events = self.ledger(id)?.read()?;
                    let open = crate::run_open_dispatch::unresolved(&events);
                    let block = events.iter().rev().find(|e| e.kind == "run_blocked");
                    if let [dispatch] = open.as_slice()
                        && block
                            .and_then(|b| b.data.get("openDispatches"))
                            .and_then(|v| v.as_array())
                            .is_some_and(|refs| {
                                refs.len() == 1
                                    && refs[0].get("dispatchId") == dispatch.data.get("dispatchId")
                            })
                    {
                        let mission = crate::Mission::parse(&std::fs::read_to_string(
                            runs.join(format!("{id}.snapshot.toml")),
                        )?)
                        .map_err(|e| io::Error::other(e.to_string()))?;
                        crate::run_reattach::stage_completed(self, id, &mission, dispatch);
                    }
                }
                continue;
            }
            let events = self.ledger(id)?.read()?;
            if let Some(pending) = events.iter().rev().find(|pending| {
                pending.kind == "output_capture_pending"
                    && !crate::run_capture::discarded(&events, pending)
                    && !events.iter().any(|event| {
                        (event.kind == "node_output" || event.kind == "node_output_pending")
                            && event.node_id == pending.node_id
                            && event.attempt == pending.attempt
                    })
            }) {
                self.append(id, crate::RunEvent {
                    kind: "run_blocked".into(), node_id: pending.node_id.clone(), actor: "archond".into(), timestamp: crate::run_storage::now(),
                    data: serde_json::json!({"code":"coordinator_shutdown","reason":"coordinator stopped while keeping outputs; resume to copy them again","blockedNodeId":pending.node_id,"resumeAllowed":true,"resumePolicy":"explicit","pendingOutput":{"nodeId":pending.node_id,"attempt":pending.attempt,"output":pending.data}}).as_object().unwrap().clone(),
                    ..Default::default()
                })?;
                interrupted.push(id.to_owned());
                continue;
            }
            let mission = crate::Mission::parse(&std::fs::read_to_string(
                runs.join(format!("{id}.snapshot.toml")),
            )?)
            .map_err(|e| io::Error::other(e.to_string()))?;
            let bindings: BindingsSnapshot = toml::from_str(&std::fs::read_to_string(
                runs.join(format!("{id}.bindings.toml")),
            )?)
            .map_err(io::Error::other)?;
            let events = self.ledger(id)?.read()?;
            let formation_id = events[0]
                .data
                .get("formationId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let start = RunStart {
                mission: projection.mission_slug,
                input_card_id: projection.input_card_id,
                formation_id,
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
            let finalizing = events
                .iter()
                .rev()
                .take_while(|event| event.kind != "run_resumed" && event.kind != "run_started")
                .any(|event| {
                    event.kind == "seat_cleanup"
                        && event.data.get("cause").and_then(|v| v.as_str()) == Some("run_final")
                });
            let open = crate::run_open_dispatch::unresolved(&events);
            let mut recovery_error = None;
            if !finalizing
                && let [dispatch] = open.as_slice()
                && !events.iter().any(|error| {
                    error.kind == "error"
                        && error.node_id == dispatch.node_id
                        && error.seq > dispatch.seq
                })
                && let Err(error) = crate::run_reattach::recover_completed(
                    self, id, &mission, &start, &workspace, &control, dispatch,
                )
            {
                recovery_error = Some(error.to_string());
            }
            let events = self.ledger(id)?.read()?;
            let projection = self.run(id).map_err(crate::error::to_io)?;
            if finalizing || !only_waiting(&mission, &events, &projection) {
                let open: Vec<_> = crate::run_open_dispatch::unresolved(&events)
                    .into_iter()
                    .filter(|dispatch| {
                        !crate::run_open_dispatch::failed_completion(&events, dispatch)
                    })
                    .collect();
                let pending = pending_capture(&events);
                let restarted = recovery_error.is_none();
                let code = if let Some(error) = &recovery_error {
                    publication_error_code(error)
                } else if finalizing {
                    "run_finalization_interrupted"
                } else {
                    "coordinator_interrupted"
                };
                if !finalizing {
                    self.append(id, crate::RunEvent { kind:"error".into(), timestamp:crate::run_storage::now(), data:serde_json::json!({"code":"coordinator_interrupted","message":if open.is_empty() {"coordinator restarted between steps"} else {"coordinator restarted with open dispatches"},"openDispatches":open.iter().map(|e| &e.data).collect::<Vec<_>>()}).as_object().unwrap().clone(), ..Default::default() })?;
                }
                self.append(id, crate::RunEvent {
                    kind: "run_blocked".into(),
                    timestamp: crate::run_storage::now(),
                    data: serde_json::json!({"code":code,"reason":recovery_error.unwrap_or_else(|| if open.is_empty() {"coordinator restarted between steps; resume to continue".into()} else {"coordinator restarted; completed-turn evidence required".into()}),"resumeAllowed":true,"resumePolicy":"explicit","openDispatches":open.iter().map(|e| &e.data).collect::<Vec<_>>(),"pendingOutput":pending}).as_object().unwrap().clone(),
                    ..Default::default()
                })?;
                if restarted {
                    interrupted.push(id.to_owned());
                }
                continue;
            }
            self.inner
                .controls
                .lock()
                .unwrap()
                .insert(id.into(), control.clone());
            let owner = self.clone();
            let run_id = id.to_owned();
            std::thread::spawn(move || {
                crate::run_worker::execute(
                    owner, run_id, mission, bindings, start, workspace, control,
                )
            });
        }
        for id in interrupted {
            self.resume_after_restart(&id);
        }
        Ok(())
    }

    /// A restart is not a decision: reattach what the restart cut short, as
    /// `run resume` would. When the evidence is missing, the resume blocks the
    /// run again and choices that discard work stay the operator's.
    fn resume_after_restart(&self, id: &str) {
        let request = crate::ResumeRequest {
            actor: "parvi:restart".into(),
            ..Default::default()
        };
        if let Err(error) = self.resume(id, request) {
            eprintln!("run {id} stays blocked after restart: {error}");
        }
    }
}

pub(crate) fn publication_error_code(detail: &str) -> &'static str {
    [
        "output_not_kept",
        "invalid_output_ref",
        "unavailable_output_ref",
    ]
    .into_iter()
    .find(|code| detail.contains(code))
    .unwrap_or("workspace_publication_failed")
}

pub(crate) fn pending_capture(events: &[crate::RunEvent]) -> Option<serde_json::Value> {
    events.iter().rev().find(|event| event.kind == "output_capture_pending" && !crate::run_capture::discarded(events, event) && !events.iter().any(|output| matches!(output.kind.as_str(), "node_output" | "node_output_pending") && output.node_id == event.node_id && output.attempt == event.attempt)).map(|event| serde_json::json!({"nodeId":event.node_id,"attempt":event.attempt,"output":event.data}))
}

pub(crate) fn only_waiting(
    mission: &crate::Mission,
    events: &[crate::RunEvent],
    projection: &crate::RunProjection,
) -> bool {
    let replay = crate::run_replay::Replay::read(mission, events);
    let unrouted_verdict = events.iter().any(|decision| {
        decision.kind == "human_verdict_recorded"
            && !events.iter().any(|routed| {
                routed.kind == "gate_verdict"
                    && routed.gate_id == decision.gate_id
                    && routed.data.get("requestedSeq") == decision.data.get("requestedSeq")
            })
    });
    let ready = replay.ports.iter().any(|(node, ports)| {
        mission.formation(node).is_some_and(|formation| {
            formation.inputs.iter().all(|p| ports.contains_key(&p.id))
                && ports.values().any(|(_, unread)| *unread)
        })
    });
    !projection.waiting_gates.is_empty()
        && replay.open.is_empty()
        && replay.evaluating.is_empty()
        && replay.queue.values().all(|q| q.is_empty())
        && replay.unfinished.is_empty()
        && !unrouted_verdict
        && !ready
}
