//! Rebuild pending deliveries and open work in ledger order.
use crate::graph::{endpoint_node, endpoint_port, judge_chain, outgoing, outgoing_from_port};
use crate::{GateFeedback, GateResponse, RoutedInput, RunEvent, merge_port_inputs};
use parvi_mission_model::{Mission, PublicOrigin};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

#[derive(Default)]
pub(crate) struct Replay {
    pub ports: BTreeMap<String, BTreeMap<String, (RoutedInput, bool)>>,
    pub queue: BTreeMap<String, VecDeque<RoutedInput>>,
    pub attempts: BTreeMap<String, i64>,
    pub open: BTreeMap<String, RunEvent>,
    pub evaluating: BTreeMap<String, RunEvent>,
    pub ended: Vec<(String, String, String, String, String)>,
    pub waiting: BTreeMap<u64, Vec<String>>,
    pub unfinished: BTreeSet<String>,
    /// Gates picking a step's best attempt: they take no new input until
    /// the pick ends.
    pub picking: BTreeSet<String>,
}

impl Replay {
    pub fn read(mission: &Mission, events: &[RunEvent]) -> Self {
        let mut state = Self::default();
        let judges: HashSet<_> = mission
            .gates
            .iter()
            .flat_map(|g| judge_chain(mission, &g.id))
            .map(|f| f.id.as_str())
            .collect();
        for event in events {
            match event.kind.as_str() {
                "node_started"
                    if mission.formation(&event.node_id).is_some()
                        && !judges.contains(event.node_id.as_str()) =>
                {
                    state.attempts.insert(event.node_id.clone(), event.attempt);
                    if !state.open.contains_key(&event.node_id)
                        && let Some(ports) = state.ports.get_mut(&event.node_id)
                    {
                        for (_, unread) in ports.values_mut() {
                            *unread = false;
                        }
                    }
                    state.open.insert(event.node_id.clone(), event.clone());
                }
                "node_output" => {
                    state.open.remove(&event.node_id);
                    if judges.contains(event.node_id.as_str()) {
                        continue;
                    }
                    if let Some(formation) = mission.formation(&event.node_id) {
                        let outputs = event
                            .data
                            .get("outputs")
                            .and_then(|value| value.as_object());
                        if formation.outputs.iter().any(|port| {
                            !outputs.is_some_and(|outputs| outputs.contains_key(&port.id))
                        }) {
                            state.unfinished.insert(event.node_id.clone());
                        } else {
                            state.unfinished.remove(&event.node_id);
                        }
                    }
                    for edge in outgoing(mission, &event.node_id) {
                        let port = endpoint_port(&edge.from);
                        let Some(payload) = event.data.get("outputs").and_then(|v| v.get(port))
                        else {
                            continue;
                        };
                        let input = RoutedInput {
                            edge_id: edge.id.clone(),
                            from_node_id: event.node_id.clone(),
                            from_port_id: port.into(),
                            to_port_id: endpoint_port(&edge.to).into(),
                            text: payload["text"].as_str().unwrap_or("").into(),
                            r#ref: payload["ref"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .map(str::to_owned)
                                .unwrap_or_else(|| {
                                    format!("ledger://{}/{}", event.run_id, edge.id)
                                }),
                            report_ref: payload["reportRef"].as_str().unwrap_or("").into(),
                            artifact_ref: payload["artifactRef"].as_str().unwrap_or("").into(),
                            kind: payload["kind"].as_str().unwrap_or("").into(),
                            origin: Some(PublicOrigin {
                                node_id: event.node_id.clone(),
                                port_id: port.into(),
                                attempt: event.attempt,
                            }),
                            ..Default::default()
                        };
                        state.deliver(mission, &edge.to, input, "", "", event);
                    }
                }
                // A pick settles the step's pending send-back, as its output
                // would: the step runs no more on those inputs.
                "pick_started" => {
                    state.open.remove(&event.node_id);
                    if let Some(ports) = state.ports.get_mut(&event.node_id) {
                        for (_, unread) in ports.values_mut() {
                            *unread = false;
                        }
                    }
                    state.picking.insert(event.gate_id.clone());
                }
                "human_input_requested" if crate::run_pick::is_pick(event) => {}
                "gate_evaluating" => {
                    if let Some(queue) = state.queue.get_mut(&event.gate_id) {
                        queue.pop_front();
                    }
                    state
                        .evaluating
                        .insert(event.gate_id.clone(), event.clone());
                }
                "human_input_requested" => {
                    state.evaluating.remove(&event.gate_id);
                }
                "gate_verdict" => {
                    let pick = crate::run_pick::is_pick(event);
                    if pick {
                        state.picking.remove(&event.gate_id);
                    } else if event
                        .data
                        .get("requestedSeq")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0)
                        == 0
                    {
                        state.evaluating.remove(&event.gate_id);
                    }
                    let route = event
                        .data
                        .get("routePort")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if route != "pass" && route != "fail" {
                        continue;
                    }
                    let Ok(input) = serde_json::from_value::<RoutedInput>(
                        event.data.get("inputRef").cloned().unwrap_or_default(),
                    ) else {
                        continue;
                    };
                    let reason = event
                        .data
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let routed: HashSet<&str> = event
                        .data
                        .get("routedEdges")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str())
                        .collect();
                    for edge in outgoing_from_port(mission, &event.gate_id, route) {
                        if !routed.is_empty() && !routed.contains(edge.id.as_str()) {
                            continue;
                        }
                        let mut next = input.clone();
                        if route == "fail" {
                            next.r#ref = format!("ledger://{}/{}", event.run_id, edge.id);
                            next.kind.clear();
                            next.feedback = Some(Box::new(GateFeedback {
                                gate_id: event.gate_id.clone(),
                                gate_attempt: event.attempt,
                                verdict: "fail".into(),
                                reason: reason.into(),
                                original_ref: input.r#ref.clone(),
                                original_text: input.text.clone(),
                                original_kind: input.kind.clone(),
                                evidence: event
                                    .data
                                    .get("evidence")
                                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                                    .unwrap_or_default(),
                                earlier: input.feedback.clone(),
                            }));
                        } else if !pick
                            && let Some(seq) = event
                                .data
                                .get("requestedSeq")
                                .and_then(|v| v.as_u64())
                                .filter(|n| *n > 0)
                            && let Some(decision) = events.iter().find(|e| {
                                e.kind == "human_verdict_recorded"
                                    && e.gate_id == event.gate_id
                                    && e.data.get("requestedSeq").and_then(|v| v.as_u64())
                                        == Some(seq)
                            })
                        {
                            let text = decision
                                .data
                                .get("reason")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if !text.is_empty() {
                                next.response = Some(Box::new(GateResponse {
                                    gate_id: event.gate_id.clone(),
                                    gate_attempt: event.attempt,
                                    requested_seq: seq,
                                    decided_by: "human:operator".into(),
                                    text: text.into(),
                                    original_ref: input.r#ref.clone(),
                                    original_text: input.text.clone(),
                                    original_kind: input.kind.clone(),
                                    earlier: input.response.clone(),
                                }));
                            }
                        }
                        if route == "fail" {
                            next.edge_id = edge.id.clone();
                            next.from_node_id = event.gate_id.clone();
                            next.from_port_id = route.into();
                        }
                        next.to_port_id = endpoint_port(&edge.to).into();
                        state.deliver(mission, &edge.to, next, &event.gate_id, reason, event);
                    }
                }
                _ => {}
            }
        }
        state
    }
    fn deliver(
        &mut self,
        mission: &Mission,
        endpoint: &str,
        input: RoutedInput,
        gate: &str,
        reason: &str,
        event: &RunEvent,
    ) {
        let node = endpoint_node(endpoint);
        if mission.gate(node).is_some() {
            self.queue.entry(node.into()).or_default().push_back(input);
        } else if let Some(end) = mission.end(node) {
            self.ended.push((
                end.id.clone(),
                end.outcome.clone(),
                gate.into(),
                if gate.is_empty() {
                    format!("the path ended at {} (rejected)", end.title)
                } else {
                    reason.into()
                },
                event.actor.clone(),
            ));
        } else if mission.formation(node).is_some() {
            let port = endpoint_port(endpoint).to_owned();
            let ports = self.ports.entry(node.into()).or_default();
            let next = match ports.remove(&port) {
                Some((older, true)) => merge_port_inputs(older, input),
                _ => input,
            };
            ports.insert(port, (next, true));
            if let Some(formation) = mission.formation(node)
                && formation
                    .inputs
                    .iter()
                    .any(|input| !ports.contains_key(&input.id))
            {
                let waiting = self.waiting.entry(event.seq).or_default();
                if !waiting.iter().any(|id| id == node) {
                    waiting.push(node.into());
                }
            }
        }
    }
}

pub(crate) fn note_waiting(
    c: &crate::Coordinator,
    id: &str,
    control: &crate::coordinator::Control,
    mission: &Mission,
    replay: &Replay,
    range: (u64, u64),
) -> std::io::Result<bool> {
    use crate::run_worker::event;
    use serde_json::json;
    fn append(
        c: &crate::Coordinator,
        id: &str,
        control: &crate::coordinator::Control,
        event: RunEvent,
    ) -> std::io::Result<()> {
        crate::run_worker::append_live(c, id, control, event).map(|_| ())
    }
    let mut noted_waiting = false;
    for (_, nodes) in replay
        .waiting
        .iter()
        .filter(|(seq, _)| **seq > range.0 && **seq <= range.1)
    {
        for node in nodes {
            let Some(formation) = mission.formation(node) else {
                continue;
            };
            let Some(ports) = replay.ports.get(node) else {
                continue;
            };
            let missing: Vec<_> = formation
                .inputs
                .iter()
                .filter(|p| !ports.contains_key(&p.id))
                .map(|p| p.id.clone())
                .collect();
            if missing.is_empty() {
                continue;
            }
            append(
                c,
                id,
                control,
                event(
                    "node_waiting",
                    node,
                    0,
                    json!({"neededInputs":missing.len(),"readyInputs":ports.len(),"totalInputs":formation.inputs.len(),"waitingFor":missing}),
                ),
            )?;
            noted_waiting = true;
        }
    }
    Ok(noted_waiting)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_mission_model::{Connection, Formation, Gate, Port};
    use serde_json::json;
    fn ev(seq: u64, kind: &str, node: &str, data: serde_json::Value) -> RunEvent {
        RunEvent {
            seq,
            run_id: "run_test".into(),
            kind: kind.into(),
            node_id: node.into(),
            data: data.as_object().unwrap().clone(),
            ..Default::default()
        }
    }
    #[test]
    fn a_delivery_during_work_stays_unread_after_its_output() {
        let mission = Mission {
            formations: vec![Formation {
                id: "work".into(),
                inputs: vec![Port {
                    id: "in".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            connections: vec![Connection {
                id: "edge".into(),
                from: "input:out".into(),
                to: "work:in".into(),
            }],
            ..Default::default()
        };
        let output = |seq, text| {
            ev(
                seq,
                "node_output",
                "input",
                json!({"outputs":{"out":{"text":text}}}),
            )
        };
        let started = ev(
            2,
            "node_started",
            "work",
            json!({"inputRefs":[{"text":"first"}]}),
        );
        let replay = Replay::read(
            &mission,
            &[
                output(1, "first"),
                started,
                output(3, "second"),
                ev(4, "node_output", "work", json!({"outputs":{}})),
            ],
        );
        assert_eq!(replay.ports["work"]["in"].0.text, "second");
        assert!(replay.ports["work"]["in"].1);
        assert!(!replay.open.contains_key("work"));
    }
    #[test]
    fn replay_uses_the_edges_recorded_by_the_verdict() {
        let mission = Mission {
            gates: vec![Gate {
                id: "gate".into(),
                ..Default::default()
            }],
            formations: ["a", "b"]
                .into_iter()
                .map(|id| Formation {
                    id: id.into(),
                    inputs: vec![Port {
                        id: "in".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                })
                .collect(),
            connections: vec![
                Connection {
                    id: "edge_a".into(),
                    from: "gate:pass".into(),
                    to: "a:in".into(),
                },
                Connection {
                    id: "edge_b".into(),
                    from: "gate:pass".into(),
                    to: "b:in".into(),
                },
            ],
            ..Default::default()
        };
        let mut verdict = ev(
            1,
            "gate_verdict",
            "gate",
            json!({"routePort":"pass","routedEdges":["edge_b"],"inputRef":{"text":"work"}}),
        );
        verdict.gate_id = "gate".into();
        let replay = Replay::read(&mission, &[verdict]);
        assert!(!replay.ports.contains_key("a"));
        assert_eq!(replay.ports["b"]["in"].0.text, "work");
    }
    #[test]
    fn rejected_end_keeps_an_empty_gate_reason() {
        let mission = Mission {
            gates: vec![Gate {
                id: "gate".into(),
                ..Default::default()
            }],
            ends: vec![parvi_mission_model::End {
                id: "end".into(),
                title: "No".into(),
                outcome: "rejected".into(),
            }],
            connections: vec![Connection {
                id: "fail".into(),
                from: "gate:fail".into(),
                to: "end:in".into(),
            }],
            ..Default::default()
        };
        let mut verdict = ev(
            1,
            "gate_verdict",
            "gate",
            json!({"routePort":"fail","reason":"","inputRef":{"text":"work"}}),
        );
        verdict.gate_id = "gate".into();
        let replay = Replay::read(&mission, &[verdict]);
        assert_eq!(replay.ended[0].3, "");
    }
    #[test]
    fn routing_an_old_human_verdict_keeps_a_newer_evaluation_open() {
        let mission = Mission {
            gates: vec![Gate {
                id: "gate".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let gate_event = |seq, kind, data| {
            let mut event = ev(seq, kind, "gate", data);
            event.gate_id = "gate".into();
            event
        };
        let events = vec![
            gate_event(1, "gate_evaluating", json!({"inputRef":{"text":"first"}})),
            gate_event(
                2,
                "human_input_requested",
                json!({"inputRef":{"text":"first"}}),
            ),
            gate_event(
                3,
                "human_verdict_recorded",
                json!({"requestedSeq":2,"verdict":"pass","reason":"ok"}),
            ),
            gate_event(4, "gate_evaluating", json!({"inputRef":{"text":"new"}})),
            gate_event(
                5,
                "gate_verdict",
                json!({"requestedSeq":2,"verdict":"pass","reason":"ok","routePort":"pass","inputRef":{"text":"first"}}),
            ),
        ];
        assert_eq!(Replay::read(&mission, &events).evaluating["gate"].seq, 4);
    }
}
