//! Resolve mission results by following accepted deliveries back from done Ends.
use crate::graph::{endpoint_node, endpoint_port, outgoing, outgoing_from_port};
use crate::{Ledger, RoutedInput, RunEvent};
use parvi_mission_model::{Mission, PublicOrigin, RunOutput};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
};

#[derive(Debug)]
pub(crate) enum MissingOutput {
    Ledger(io::Error),
    Missing(String),
}

/// Resolve declared outputs from the persisted ledger. Rejected gate inputs do
/// not contribute their origins, even when a later attempt consumed feedback.
pub fn resolve(ledger: &Ledger, mission: &Mission) -> Result<Vec<RunOutput>, crate::Error> {
    let events = ledger.read()?;
    resolve_events(&events, mission)
        .map(|resolved| resolved.outputs)
        .map_err(|error| match error {
            MissingOutput::Ledger(error) => crate::error::io(error),
            MissingOutput::Missing(message) => crate::Error::new("missing_output", message),
        })
}

pub(crate) struct Resolved {
    pub outputs: Vec<RunOutput>,
    pub picks: Vec<serde_json::Value>,
}
pub(crate) fn resolve_with_picks(
    ledger: &Ledger,
    mission: &Mission,
) -> Result<Resolved, MissingOutput> {
    let events = ledger
        .read()
        .map_err(|error| MissingOutput::Ledger(crate::error::to_io(error)))?;
    resolve_events(&events, mission)
}

fn resolve_events(events: &[RunEvent], mission: &Mission) -> Result<Resolved, MissingOutput> {
    type Key = (String, i64);
    type Origin = (String, String, i64);
    let judges: HashSet<_> = mission
        .gates
        .iter()
        .flat_map(|gate| crate::graph::judge_chain(mission, &gate.id))
        .map(|formation| formation.id.as_str())
        .collect();
    let mut inputs: HashMap<Key, Vec<RoutedInput>> = HashMap::new();
    let mut produced = HashMap::new();
    let mut queue = VecDeque::<Origin>::new();
    let done = |to: &str| {
        mission
            .end(endpoint_node(to))
            .is_some_and(|end| end.outcome != "rejected")
    };
    let follow = |input: &RoutedInput, queue: &mut VecDeque<Origin>| {
        if !(mission.gate(&input.from_node_id).is_some() && input.from_port_id == "fail")
            && let Some(PublicOrigin {
                node_id,
                port_id,
                attempt,
            }) = &input.origin
        {
            queue.push_back((node_id.clone(), port_id.clone(), *attempt));
        }
    };
    for event in events {
        let key = (event.node_id.clone(), event.attempt);
        match event.kind.as_str() {
            "node_started" => {
                inputs.insert(
                    key,
                    event
                        .data
                        .get("inputRefs")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default(),
                );
            }
            "node_output" if !judges.contains(event.node_id.as_str()) => {
                if let Some(payloads) = event.data.get("outputs").and_then(|v| v.as_object()) {
                    for edge in outgoing(mission, &event.node_id).filter(|edge| done(&edge.to)) {
                        let port = endpoint_port(&edge.from);
                        if payloads.contains_key(port) {
                            queue.push_back((event.node_id.clone(), port.into(), event.attempt));
                        }
                    }
                    produced.insert(key, payloads.clone());
                }
            }
            "gate_verdict"
                if event.data.get("routePort").and_then(|v| v.as_str()) == Some("pass") =>
            {
                let routes = event.data.get("routedEdges").and_then(|v| v.as_array());
                let delivered = outgoing_from_port(mission, &event.gate_id, "pass").any(|edge| {
                    done(&edge.to)
                        && routes.is_none_or(|r| {
                            r.is_empty() || r.iter().any(|v| v.as_str() == Some(&edge.id))
                        })
                });
                if delivered
                    && let Some(input) = event
                        .data
                        .get("inputRef")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                {
                    follow(&input, &mut queue);
                }
            }
            _ => {}
        }
    }
    let roots = queue.clone();
    let mut reached = HashMap::<(String, String), (i64, serde_json::Value)>::new();
    let mut seen = HashSet::new();
    while let Some((node, port, attempt)) = queue.pop_front() {
        if !seen.insert((node.clone(), port.clone(), attempt)) {
            continue;
        }
        let key = (node.clone(), attempt);
        let Some(payload) = produced.get(&key).and_then(|p| p.get(&port)) else {
            continue;
        };
        let entry = reached
            .entry((node, port))
            .or_insert((attempt, payload.clone()));
        if attempt > entry.0 {
            *entry = (attempt, payload.clone());
        }
        if let Some(refs) = inputs.get(&key) {
            for input in refs {
                follow(input, &mut queue);
            }
        }
    }
    let picks = picked(events);
    let mut run_picks: Vec<_> = roots
        .iter()
        .cloned()
        .flat_map(|origin| inherited_picks(origin, &inputs, &picks, mission))
        .collect();
    run_picks.sort_by_key(|pick| pick["seq"].as_u64().unwrap_or(0));
    run_picks.dedup_by_key(|pick| pick["seq"].as_u64().unwrap_or(0));
    let mut result = Vec::new();
    let mut problems = Vec::new();
    for output in mission
        .input_card()
        .into_iter()
        .flat_map(|card| &card.outputs)
    {
        let (node, port) = output.from.split_once(':').unwrap_or(("", ""));
        let kind = if output.kind.is_empty() {
            "text"
        } else {
            &output.kind
        };
        let found = reached.get(&(node.to_owned(), port.to_owned()));
        let value = found
            .and_then(|(_, p)| p.get(if kind == "text" { "text" } else { "ref" }))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if value.trim().is_empty() {
            if output.required {
                problems.push(format!(
                    "required output {}: no output of {} reached a done End",
                    output.name, output.from
                ));
            }
            continue;
        }
        if kind != "text"
            && !std::fs::metadata(value).is_ok_and(|m| {
                if kind == "file" {
                    m.is_file()
                } else {
                    m.is_dir()
                }
            })
        {
            problems.push(format!(
                "output {}: {} is unavailable or is not a {}",
                output.name, value, kind
            ));
            continue;
        }
        result.push(RunOutput {
            name: output.name.clone(),
            kind: kind.into(),
            value: value.into(),
            node_id: node.into(),
            port_id: port.into(),
            attempt: found.unwrap().0,
            picks: inherited_picks(
                (node.to_owned(), port.to_owned(), found.unwrap().0),
                &inputs,
                &picks,
                mission,
            ),
        });
    }
    // A mission that declares no outputs still produced something: what
    // reached a done End, named after the step that made it.
    if mission
        .input_card()
        .is_none_or(|card| card.outputs.is_empty())
    {
        let mut ends: Vec<(String, String)> = roots
            .iter()
            .map(|(node, port, _)| (node.clone(), port.clone()))
            .collect();
        ends.dedup();
        for (node, port) in ends {
            let Some((attempt, payload)) = reached.get(&(node.clone(), port.clone())) else {
                continue;
            };
            let text = |key| payload.get(key).and_then(|v| v.as_str()).unwrap_or("");
            let (kind, value) = match text("ref") {
                "" => ("text", text("text")),
                path if std::path::Path::new(path).is_dir() => ("folder", path),
                path => ("file", path),
            };
            if value.trim().is_empty() || result.iter().any(|o: &RunOutput| o.value == value) {
                continue;
            }
            let name = mission
                .formations
                .iter()
                .find(|f| f.id == node)
                .map_or(node.clone(), |f| f.title.clone());
            result.push(RunOutput {
                name,
                kind: kind.into(),
                value: value.into(),
                node_id: node.clone(),
                port_id: port.clone(),
                attempt: *attempt,
                picks: inherited_picks((node, port, *attempt), &inputs, &picks, mission),
            });
        }
    }
    if problems.is_empty() {
        Ok(Resolved {
            outputs: result,
            picks: run_picks,
        })
    } else {
        Err(MissingOutput::Missing(problems.join("; ")))
    }
}

type Origin = (String, String, i64);
fn picked(events: &[RunEvent]) -> HashMap<Origin, serde_json::Value> {
    let mut result = HashMap::new();
    for event in events.iter().filter(|e| e.kind == "gate_verdict") {
        let Some(pick) = event.data.get("pick") else {
            continue;
        };
        let Some(origin) = event
            .data
            .get("inputRef")
            .and_then(|v| v.get("origin"))
            .and_then(|v| serde_json::from_value::<PublicOrigin>(v.clone()).ok())
        else {
            continue;
        };
        let attempt = pick.get("attempt").and_then(|v| v.as_i64()).unwrap_or(0);
        let started = pick
            .get("pickSeq")
            .and_then(|v| v.as_u64())
            .and_then(|seq| events.iter().find(|e| e.seq == seq));
        let verdict = started
            .and_then(|e| e.data.get("candidates"))
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .find(|c| c.get("attempt").and_then(|v| v.as_i64()) == Some(attempt))
            .and_then(|c| c.get("verdictSeq"))
            .and_then(|v| v.as_u64())
            .and_then(|seq| events.iter().find(|e| e.seq == seq));
        let reason = verdict
            .and_then(|e| e.data.get("reason"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let findings: Vec<_> = verdict
            .and_then(|e| e.data.get("evidence"))
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|e| e.get("text").and_then(|v| v.as_str()))
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != reason)
            .collect();
        let mut detail = serde_json::json!({"seq":verdict.map_or(0, |v| v.seq),"verdict":verdict.and_then(|v| v.data.get("verdict")).and_then(|v| v.as_str()).unwrap_or(""),"reason":reason});
        if !findings.is_empty() {
            detail["findings"] = serde_json::json!(findings);
        }
        result.insert((origin.node_id, origin.port_id, origin.attempt), serde_json::json!({"nodeId":pick.get("nodeId").and_then(|v| v.as_str()).unwrap_or(""),"attempt":attempt,"gateId":event.gate_id,"seq":event.seq,"decidedBy":pick.get("decidedBy").and_then(|v| v.as_str()).unwrap_or(""),"verdict":detail}));
    }
    result
}
fn inherited_picks(
    origin: Origin,
    inputs: &HashMap<(String, i64), Vec<RoutedInput>>,
    picks: &HashMap<Origin, serde_json::Value>,
    mission: &Mission,
) -> Vec<serde_json::Value> {
    let mut queue = VecDeque::from([origin]);
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    while let Some(origin) = queue.pop_front() {
        if !seen.insert(origin.clone()) {
            continue;
        }
        if let Some(pick) = picks.get(&origin) {
            result.push(pick.clone());
        }
        for input in inputs
            .get(&(origin.0.clone(), origin.2))
            .into_iter()
            .flatten()
        {
            if mission.gate(&input.from_node_id).is_some() && input.from_port_id == "fail" {
                continue;
            }
            if let Some(o) = &input.origin {
                queue.push_back((o.node_id.clone(), o.port_id.clone(), o.attempt));
            }
        }
    }
    result.sort_by_key(|pick| pick["seq"].as_u64().unwrap_or(0));
    result.dedup_by_key(|pick| pick["seq"].as_u64().unwrap_or(0));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_mission_model::{Connection, End, Gate, InputCard, MissionOutput};
    use serde_json::json;

    fn event(kind: &str, node: &str, attempt: i64, data: serde_json::Value) -> RunEvent {
        RunEvent {
            kind: kind.into(),
            node_id: node.into(),
            attempt,
            data: data.as_object().unwrap().clone(),
            ..Default::default()
        }
    }

    #[test]
    fn follows_accepted_work_but_not_rejected_feedback() {
        let mission = Mission {
            input_cards: vec![InputCard {
                outputs: vec![MissionOutput {
                    name: "answer".into(),
                    from: "work:out".into(),
                    required: true,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            gates: vec![Gate {
                id: "gate".into(),
                ..Default::default()
            }],
            ends: vec![End {
                id: "end".into(),
                outcome: "done".into(),
                ..Default::default()
            }],
            connections: vec![Connection {
                id: "done".into(),
                from: "gate:pass".into(),
                to: "end:in".into(),
            }],
            ..Default::default()
        };
        let mut verdict = event(
            "gate_verdict",
            "gate",
            1,
            json!({"routePort":"pass","inputRef":{"origin":{"nodeId":"work","portId":"out","attempt":2}}}),
        );
        verdict.gate_id = "gate".into();
        let events = vec![
            event("node_started", "work", 1, json!({"inputRefs":[]})),
            event(
                "node_output",
                "work",
                1,
                json!({"outputs":{"out":{"text":"rejected"}}}),
            ),
            event(
                "node_started",
                "work",
                2,
                json!({"inputRefs":[{"fromNodeId":"gate","fromPortId":"fail","origin":{"nodeId":"work","portId":"out","attempt":1}}]}),
            ),
            event(
                "node_output",
                "work",
                2,
                json!({"outputs":{"out":{"text":"fixed"}}}),
            ),
            verdict,
        ];
        let outputs = resolve_events(&events, &mission).unwrap().outputs;
        assert_eq!(outputs[0].value, "fixed");
        assert_eq!(outputs[0].attempt, 2);
    }
    #[test]
    fn a_mission_without_declared_outputs_shows_what_reached_its_end() {
        let mission = Mission {
            formations: vec![parvi_mission_model::Formation {
                id: "write".into(),
                title: "Write the plan".into(),
                ..Default::default()
            }],
            ends: vec![End {
                id: "done".into(),
                outcome: "done".into(),
                ..Default::default()
            }],
            connections: vec![Connection {
                id: "finish".into(),
                from: "write:out".into(),
                to: "done:in".into(),
            }],
            ..Default::default()
        };
        let events = vec![event(
            "node_output",
            "write",
            1,
            json!({"outputs":{"out":{"text":"the plan"}}}),
        )];
        let outputs = resolve_events(&events, &mission).unwrap().outputs;
        assert_eq!(outputs.len(), 1);
        assert_eq!(
            (outputs[0].name.as_str(), outputs[0].kind.as_str()),
            ("Write the plan", "text")
        );
        assert_eq!(outputs[0].value, "the plan");
    }
    #[test]
    fn only_done_paths_contribute_and_blank_required_output_fails() {
        let mut mission = Mission {
            input_cards: vec![InputCard {
                outputs: vec![MissionOutput {
                    name: "answer".into(),
                    from: "work:out".into(),
                    required: true,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            gates: vec![Gate {
                id: "gate".into(),
                ..Default::default()
            }],
            ends: vec![End {
                id: "done".into(),
                outcome: "done".into(),
                ..Default::default()
            }],
            connections: vec![Connection {
                id: "done_edge".into(),
                from: "fix:out".into(),
                to: "done:in".into(),
            }],
            ..Default::default()
        };
        let events = vec![
            event(
                "node_output",
                "work",
                1,
                json!({"outputs":{"out":{"text":"rejected"}}}),
            ),
            event(
                "node_started",
                "fix",
                1,
                json!({"inputRefs":[{"fromNodeId":"gate","fromPortId":"fail","origin":{"nodeId":"work","portId":"out","attempt":1}}]}),
            ),
            event(
                "node_output",
                "fix",
                1,
                json!({"outputs":{"out":{"text":"fixed"}}}),
            ),
        ];
        assert!(
            matches!(resolve_events(&events, &mission), Err(MissingOutput::Missing(reason)) if reason.contains("required output answer"))
        );
        mission.input_cards[0].outputs[0].required = false;
        assert!(
            resolve_events(&events, &mission)
                .unwrap()
                .outputs
                .is_empty()
        );
    }
    #[test]
    fn carries_pick_provenance_through_later_attempts() {
        let mission = Mission {
            input_cards: vec![InputCard {
                outputs: vec![MissionOutput {
                    name: "answer".into(),
                    from: "fix:out".into(),
                    required: true,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ends: vec![End {
                id: "done".into(),
                outcome: "done".into(),
                ..Default::default()
            }],
            connections: vec![Connection {
                id: "done_edge".into(),
                from: "fix:out".into(),
                to: "done:in".into(),
            }],
            ..Default::default()
        };
        let mut verdict = event(
            "gate_verdict",
            "gate",
            1,
            json!({"verdict":"fail","reason":"bad","evidence":[{"text":"bad"},{"text":"missing test"}]}),
        );
        verdict.seq = 1;
        let mut started = event(
            "pick_started",
            "work",
            0,
            json!({"candidates":[{"attempt":1,"verdictSeq":1}]}),
        );
        started.seq = 2;
        let mut picked = event(
            "gate_verdict",
            "gate",
            2,
            json!({"pick":{"nodeId":"work","attempt":1,"pickSeq":2,"decidedBy":"operator"},"inputRef":{"origin":{"nodeId":"work","portId":"out","attempt":1}}}),
        );
        picked.seq = 3;
        picked.gate_id = "gate".into();
        let events = vec![
            verdict,
            started,
            picked,
            event(
                "node_started",
                "fix",
                1,
                json!({"inputRefs":[{"origin":{"nodeId":"work","portId":"out","attempt":1}}]}),
            ),
            event(
                "node_output",
                "fix",
                1,
                json!({"outputs":{"out":{"text":"fixed"}}}),
            ),
        ];
        let result = resolve_events(&events, &mission).unwrap().outputs;
        assert_eq!(
            result[0].picks[0]["verdict"]["findings"],
            json!(["missing test"])
        );
        assert_eq!(result[0].picks[0]["seq"], 3);
    }
    #[test]
    fn done_end_fix_and_missing_match_archon_lab() {
        let expected: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/parity/runs/archon-ledgers/outputs.json"
        ))
        .unwrap();
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let root = std::env::temp_dir().join(format!(
            "parvi-102-output-parity-{:x}",
            u64::from_ne_bytes(random)
        ));
        let work = root.join("fmn_work/1/outputs/port_work_out/site");
        let fix = root.join("fmn_fix/1/outputs/port_fix_out/site");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&fix).unwrap();
        for (case, bound, use_fix) in [
            ("outputs-done-end", "work", false),
            ("outputs-fixed-after-fail", "fix", true),
            ("missing-required-output", "fix", false),
        ] {
            let name = if bound == "work" { "site" } else { "fixed" };
            let mission = Mission {
                input_cards: vec![InputCard {
                    outputs: vec![MissionOutput {
                        name: name.into(),
                        kind: "folder".into(),
                        from: format!("fmn_{bound}:port_{bound}_out"),
                        required: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                gates: vec![Gate {
                    id: "gate_lint".into(),
                    ..Default::default()
                }],
                ends: vec![End {
                    id: "done".into(),
                    outcome: "done".into(),
                    ..Default::default()
                }],
                connections: vec![
                    Connection {
                        id: "after".into(),
                        from: "fmn_after:out".into(),
                        to: "done:in".into(),
                    },
                    Connection {
                        id: "fix".into(),
                        from: "fmn_fix:port_fix_out".into(),
                        to: "done:in".into(),
                    },
                ],
                ..Default::default()
            };
            let mut events = vec![event(
                "node_output",
                "fmn_work",
                1,
                json!({"outputs":{"port_work_out":{"ref":work,"text":"work"}}}),
            )];
            if use_fix {
                events.push(event("node_started", "fmn_fix", 1, json!({"inputRefs":[{"fromNodeId":"gate_lint","fromPortId":"fail","origin":{"nodeId":"fmn_work","portId":"port_work_out","attempt":1}}]})));
                events.push(event(
                    "node_output",
                    "fmn_fix",
                    1,
                    json!({"outputs":{"port_fix_out":{"ref":fix,"text":"fixed"}}}),
                ));
            } else {
                events.push(event("node_started", "fmn_after", 1, json!({"inputRefs":[{"fromNodeId":"fmn_work","fromPortId":"port_work_out","origin":{"nodeId":"fmn_work","portId":"port_work_out","attempt":1}}]})));
                events.push(event(
                    "node_output",
                    "fmn_after",
                    1,
                    json!({"outputs":{"out":{"text":"shipped"}}}),
                ));
            }
            let actual = match resolve_events(&events, &mission) {
                Ok(resolved) => {
                    let mut outputs = serde_json::to_value(resolved.outputs).unwrap();
                    for output in outputs.as_array_mut().unwrap() {
                        let path = std::path::Path::new(output["value"].as_str().unwrap());
                        let relative = path
                            .strip_prefix(&root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/");
                        output["value"] = json!(format!("<artifacts>/{relative}"));
                    }
                    json!({"status":"succeeded","code":null,"outputs":outputs})
                }
                Err(MissingOutput::Missing(_)) => {
                    json!({"status":"failed","code":"missing_output","outputs":[]})
                }
                Err(e) => panic!("{e:?}"),
            };
            assert_eq!(actual, expected[case], "{case}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn picks_reaching_a_done_end_are_independent_of_declared_outputs() {
        let mut mission = Mission {
            gates: vec![Gate {
                id: "gate".into(),
                ..Default::default()
            }],
            ends: vec![End {
                id: "done".into(),
                outcome: "done".into(),
                ..Default::default()
            }],
            connections: vec![
                Connection {
                    id: "pass".into(),
                    from: "gate:pass".into(),
                    to: "done:in".into(),
                },
                Connection {
                    id: "other".into(),
                    from: "other:out".into(),
                    to: "done:in".into(),
                },
            ],
            ..Default::default()
        };
        let mut verdict = event(
            "gate_verdict",
            "gate",
            1,
            json!({"routePort":"pass","pick":{"nodeId":"work","attempt":1,"decidedBy":"operator"},"inputRef":{"origin":{"nodeId":"work","portId":"out","attempt":1}}}),
        );
        verdict.seq = 3;
        verdict.gate_id = "gate".into();
        let events = vec![
            event(
                "node_output",
                "work",
                1,
                json!({"outputs":{"out":{"text":"picked"}}}),
            ),
            verdict,
            event(
                "node_output",
                "other",
                1,
                json!({"outputs":{"out":{"text":"unrelated"}}}),
            ),
        ];
        let resolved = resolve_events(&events, &mission).unwrap();
        assert_eq!(resolved.picks.len(), 1);
        // With nothing declared, what reached the End stands as the outputs,
        // the picked one carrying its pick.
        let values: Vec<_> = resolved.outputs.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, ["picked", "unrelated"]);
        assert_eq!(resolved.outputs[0].picks.len(), 1);
        assert!(resolved.outputs[1].picks.is_empty());
        mission.input_cards.push(InputCard {
            outputs: vec![MissionOutput {
                name: "other".into(),
                from: "other:out".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let resolved = resolve_events(&events, &mission).unwrap();
        assert_eq!(resolved.picks.len(), 1);
        assert_eq!(resolved.outputs.len(), 1);
        assert!(resolved.outputs[0].picks.is_empty());
    }
}
