//! Driver-facing gate state and the consequences visible on the frozen graph.
use crate::{Coordinator, Error};
use serde_json::{Value, json};
use time::OffsetDateTime;

impl Coordinator {
    /// Every gate on the frozen mission, including gates not reached yet.
    pub fn gates(&self, id: &str) -> Result<Vec<Value>, Error> {
        let events = self.ledger(id).map_err(crate::error::io)?.read()?;
        let run = crate::project_run(&events, id)?;
        let mission = self.frozen_mission(id)?;
        Ok(mission.gates.iter().map(|gate| {
            let waiting = run.waiting_gates.iter().find(|g| g.gate_id == gate.id);
            let answer = events.iter().rev().find(|e| e.gate_id == gate.id && (e.kind == "human_verdict_recorded" || e.kind == "gate_verdict"));
            let mut result = json!({"gate":gate.id,"title":gate.title,"state":if waiting.is_some() {"waiting"} else if answer.is_some() {"answered"} else {"not_reached"}});
            if let Some(waiting) = waiting { result["requestedSeq"] = json!(waiting.requested_seq); }
            else if let Some(answer) = answer {
                let answer = answer.data.get("requestedSeq").and_then(Value::as_u64).and_then(|seq| events.iter().rev().find(|e| e.kind == "human_verdict_recorded" && e.gate_id == gate.id && e.data.get("requestedSeq").and_then(Value::as_u64) == Some(seq))).unwrap_or(answer);
                result["verdict"] = json!(answer.data.get("verdict").and_then(Value::as_str).unwrap_or(if answer.data.contains_key("pick") { "pass" } else { "" }));
                result["by"] = json!(answer.data.get("decidedBy").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(&answer.actor));
                if let Some(relay) = answer.data.get("relayedBy").and_then(Value::as_str).filter(|s| !s.is_empty()) { result["relayedBy"] = json!(relay); }
            }
            result
        }).collect())
    }

    /// A waiting gate's request and each possible route, from the frozen run.
    pub fn gate_driver_request(&self, id: &str, gate: &str) -> Result<Value, Error> {
        let events = self.ledger(id).map_err(crate::error::io)?.read()?;
        let mission = self.frozen_mission(id)?;
        let definition = mission
            .gate(gate)
            .ok_or_else(|| Error::new("not_found", "gate not found"))?;
        let request =
            crate::gate_request::pending_request(id, gate, &events, &definition.criterion)
                .map_err(|e| {
                    if e.code == "not_found" || e.code == "not_pending" {
                        Error::new("gate_not_waiting", "gate is not waiting")
                    } else {
                        e
                    }
                })?;
        let raw = events
            .iter()
            .find(|e| e.seq == request.requested_seq)
            .expect("pending request exists");
        let input = raw.data.get("inputRef").cloned().unwrap_or_default();
        let replay = crate::run_replay::Replay::read(&mission, &events);
        let projection = crate::project_run(&events, id)?;
        let routes: Vec<_> = ["pass", "fail"]
            .into_iter()
            .filter(|verdict| request.pick.is_none() || *verdict == "pass")
            .map(|verdict| {
                let mut targets = Vec::new();
                for edge in crate::graph::outgoing_from_port(&mission, gate, verdict) {
                    let node = crate::graph::endpoint_node(&edge.to);
                    let mut target =
                        json!({"nodeId":node,"portId":crate::graph::endpoint_port(&edge.to)});
                    if let Some(formation) = mission.formation(node) {
                        target["kind"] = json!("formation");
                        target["title"] = json!(formation.title);
                        let uses = crate::limits::uses_for_step(
                            &mission,
                            &events,
                            node,
                            OffsetDateTime::now_utc(),
                        );
                        target["limits"] = json!(uses.iter().map(|limit| json!({"kind":limit.kind,"limitId":limit.limit_id,"nodeId":limit.node_id,"used":limit.used,"max":limit.max,"left":(limit.max - limit.used).max(0),"granted":limit.granted})).collect::<Vec<_>>());
                        let waits = formation.inputs.iter().any(|port| {
                            !replay.ports.get(node).is_some_and(|ports| ports.contains_key(&port.id))
                                && !crate::graph::outgoing_from_port(&mission, gate, verdict).any(|edge| crate::graph::endpoint_node(&edge.to) == node && crate::graph::endpoint_port(&edge.to) == port.id)
                        });
                        target["waitsForInputs"] = json!(waits);
                        if !waits && let Some(spent) = crate::limits::spent_before(
                            &mission,
                            &events,
                            node,
                            OffsetDateTime::now_utc(),
                        ) {
                            let best = crate::limits::card(&mission, &spent.limit_id)
                                .is_some_and(|card| card.when_spent == "best")
                                && crate::run_pick::pick_for(&mission, &events, node, &spent)
                                    .is_some_and(|pick| crate::run_pick::judged_since_last_pick(&events, node, &pick.candidates));
                            target["picksBest"] = json!(best);
                            target["spentLimit"] = json!(spent);
                        }
                    } else if let Some(end) = mission.end(node) {
                        target["kind"] = json!("end");
                        target["title"] = json!(end.title);
                        target["outcome"] = json!(end.outcome);
                    } else if let Some(next) = mission.gate(node) {
                        target["kind"] = json!("gate");
                        target["title"] = json!(next.title);
                    }
                    targets.push(target);
                }
                // Only claim a final run when all destinations are ends and no
                // other active work or pending gate remains.
                let rejected = targets.iter().any(|target| target["outcome"] == "rejected") || replay.ended.iter().any(|(_, outcome, _, _, _)| outcome == "rejected");
                let starved = mission.formations.iter().any(|formation| replay.ports.get(&formation.id).is_some_and(|ports| ports.values().any(|(_, unread)| *unread) && formation.inputs.iter().any(|port| !ports.contains_key(&port.id))));
                let ready = mission.formations.iter().any(|formation| replay.ports.get(&formation.id).is_some_and(|ports| ports.values().any(|(_, unread)| *unread) && formation.inputs.iter().all(|port| ports.contains_key(&port.id))));
                let ends = !ready && (rejected || !starved) && !targets.is_empty()
                    && targets.iter().all(|t| t["kind"] == "end")
                    && projection.waiting_gates.len() == 1
                    && replay.open.is_empty()
                    && replay.evaluating.is_empty()
                    && replay.picking.iter().all(|pending| pending == gate)
                    && replay.queue.values().all(|q| q.is_empty())
                    && replay.unfinished.is_empty();
                let picks = targets.iter().any(|t| t["picksBest"] == true);
                json!({"verdict":verdict,"targets":targets,"endsRun":ends,"picksBest":picks})
            })
            .collect();
        let inputs = vec![
            json!({"fromNodeId":request.input.from_node_id,"fromPortId":request.input.from_port_id,"text":request.input.text,"truncated":request.input.truncated,"ref":input["ref"],"reportRef":input["reportRef"],"artifactRef":input["artifactRef"]}),
        ];
        Ok(
            json!({"gate":gate,"requestedSeq":request.requested_seq,"criterion":request.criterion,"inputs":inputs,"routes":routes,"endsRun":routes.iter().any(|r| r["endsRun"] == true),"picksBest":request.pick.is_some() || routes.iter().any(|r| r["picksBest"] == true),"pick":request.pick}),
        )
    }
}
