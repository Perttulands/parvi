//! Needs-you messages projected from the durable run ledger.
use crate::{Coordinator, Error, RunEvent};
use serde_json::{Value, json};

fn field<'a>(event: &'a RunEvent, name: &str) -> &'a str {
    event.data.get(name).and_then(Value::as_str).unwrap_or("")
}

impl Coordinator {
    /// Current asks, keyed by their ledger sequence. Blocks are announced only
    /// after the worker has settled, so a concurrent cancellation can win.
    pub fn notifications(&self, id: &str) -> Result<Vec<Value>, Error> {
        let events = self.ledger(id).map_err(crate::error::io)?.read()?;
        let run = crate::project_run(&events, id)?;
        let mission = self.frozen_mission(id)?;
        let settled = !self.inner.controls.lock().unwrap().contains_key(id);
        let mut asks = Vec::new();
        let last_resume = events
            .iter()
            .rev()
            .find(|e| e.kind == "run_resumed")
            .map_or(0, |e| e.seq);
        if !run.r#final {
            for gate in &run.waiting_gates {
                if let Some(event) = events.iter().find(|e| e.seq == gate.requested_seq) {
                    let definition = mission.gate(&gate.gate_id);
                    let title = definition
                        .map(|g| g.title.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&gate.gate_id);
                    let criterion = definition.map(|g| g.criterion.as_str()).unwrap_or("");
                    let input = event
                        .data
                        .get("inputRef")
                        .and_then(|v| v.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let input = &input[..input.floor_char_boundary(input.len().min(64 * 1024))];
                    let (severity, commands) = if gate.pick.is_some() {
                        let attempt = gate
                            .pick
                            .as_ref()
                            .and_then(|p| p["candidates"].as_array())
                            .and_then(|a| a.first())
                            .and_then(Value::as_i64)
                            .unwrap_or(1);
                        (
                            "pick",
                            format!(
                                "parvi gate pick {id} {} --requested-seq {} --attempt {attempt}",
                                gate.gate_id, gate.requested_seq
                            ),
                        )
                    } else {
                        (
                            "verdict",
                            format!(
                                "parvi gate approve {id} {} --requested-seq {}\nparvi gate reject {id} {} --requested-seq {}",
                                gate.gate_id, gate.requested_seq, gate.gate_id, gate.requested_seq
                            ),
                        )
                    };
                    let subject = format!("Parvi needs your answer: {title} ({})", mission.title);
                    let body = format!(
                        "{} is waiting at {title}.\n\nCriterion: {criterion}\n\nInput:\n{input}\n\n{commands}\n\nRun: {id}\nMission: {}\n",
                        mission.title, run.mission_slug
                    );
                    asks.push(message(
                        &run,
                        event,
                        &mission.title,
                        "human_gate",
                        criterion,
                        severity,
                        true,
                        title,
                        subject,
                        body,
                    ));
                }
            }
            let escalations: Vec<_> = events
                .iter()
                .filter(|e| {
                    e.kind == "escalation_raised"
                        && e.seq > last_resume
                        && e.data.get("blocks").and_then(Value::as_bool) == Some(true)
                })
                .collect();
            for event in &escalations {
                let reason = field(event, "reason");
                let subject = format!(
                    "Parvi needs you: {} escalated ({})",
                    event.node_id, mission.title
                );
                let body = format!(
                    "Run {id} needs you: {reason}\n\nparvi run resume {id} --reason 'how it was resolved'\n"
                );
                asks.push(message(
                    &run,
                    event,
                    &mission.title,
                    "escalation",
                    reason,
                    field(event, "severity"),
                    true,
                    "",
                    subject,
                    body,
                ));
            }
            if settled
                && run.status == "blocked"
                && escalations.is_empty()
                && let Some(event) = events.iter().rev().find(|e| e.kind == "run_blocked")
                && (field(event, "code") != "coordinator_shutdown" || run.waiting_gates.is_empty())
            {
                let reason = field(event, "reason");
                let subject = format!("Parvi run blocked: {}", mission.title);
                let commands = if !run.resume_allowed {
                    "Inspect the run and start a new one.".to_owned()
                } else {
                    match run.resume_policy.as_str() {
                        "grant" => format!("parvi run resume {id} --grant"),
                        "grant_or_pick" => format!(
                            "parvi run resume {id} --grant\nparvi run resume {id} --pick-best"
                        ),
                        _ => format!("parvi run resume {id}"),
                    }
                };
                let body = format!("Run {id} is blocked: {reason}\n\n{commands}\n");
                asks.push(message(
                    &run,
                    event,
                    &mission.title,
                    "blocked",
                    reason,
                    "",
                    true,
                    "",
                    subject,
                    body,
                ));
            }
        } else if settled && let Some(event) = events.last() {
            let subject = format!("Parvi run {}: {}", run.status, mission.title);
            let picked = if run.status == "picked" {
                "Work continued through a pick of failed attempts.\n"
            } else {
                ""
            };
            let body = format!(
                "Run {id} on {} finished: {}.\n{picked}{}",
                mission.title,
                run.status,
                field(event, "reason")
            );
            asks.push(message(
                &run,
                event,
                &mission.title,
                "final",
                field(event, "reason"),
                "",
                false,
                "",
                subject,
                body,
            ));
        }
        asks.sort_by_key(|a| a["seq"].as_u64().unwrap_or(0));
        Ok(asks)
    }
}

#[allow(clippy::too_many_arguments)]
fn message(
    run: &parvi_mission_model::RunProjection,
    event: &RunEvent,
    mission: &str,
    kind: &str,
    ask: &str,
    severity: &str,
    blocks: bool,
    gate_title: &str,
    subject: String,
    body: String,
) -> Value {
    json!({"runId":run.run_id,"missionSlug":run.mission_slug,"missionTitle":mission,"seq":event.seq,"kind":kind,"runStatus":run.status,"nodeId":event.node_id,"gateId":event.gate_id,"gateTitle":gate_title,"ask":ask,"severity":severity,"blocks":blocks,"text":subject,"subject":subject,"body":body})
}
