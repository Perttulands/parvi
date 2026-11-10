//! Reading a mission as a graph: endpoints, wires, reach and judge chains.

use std::collections::{HashSet, VecDeque};

use crate::mission::Mission;
use crate::nodes::{
    Connection, END_IN, Formation, GATE_FAIL, GATE_IN, GATE_JUDGE, GATE_PASS, Gate, INPUT_CARD_OUT,
};

/// Which way a port carries work.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}

/// Splits `node:port` at its first colon; both parts must be present.
pub fn split_endpoint(endpoint: &str) -> Option<(&str, &str)> {
    endpoint
        .split_once(':')
        .filter(|(node, port)| !node.is_empty() && !port.is_empty())
}

/// The node of an endpoint: everything before its first colon.
pub fn endpoint_node(endpoint: &str) -> &str {
    endpoint.split_once(':').map_or(endpoint, |(node, _)| node)
}

/// The port of an endpoint, or "" when it names none.
pub fn endpoint_port(endpoint: &str) -> &str {
    endpoint.split_once(':').map_or("", |(_, port)| port)
}

/// Whether an endpoint names an existing port that carries work this way.
/// A gate's judge port goes both ways: it sends work to its judges and takes
/// their verdict back.
pub fn endpoint_allows(mission: &Mission, endpoint: &str, direction: Direction) -> bool {
    let Some((node, port)) = split_endpoint(endpoint) else {
        return false;
    };
    if let Some(formation) = mission.formation(node) {
        let ports = match direction {
            Direction::Input => &formation.inputs,
            Direction::Output => &formation.outputs,
        };
        return ports.iter().any(|candidate| candidate.id == port);
    }
    if mission.gate(node).is_some() {
        return match port {
            GATE_IN => direction == Direction::Input,
            GATE_PASS | GATE_FAIL => direction == Direction::Output,
            GATE_JUDGE => true,
            _ => false,
        };
    }
    if mission.has_input_card(node) {
        return port == INPUT_CARD_OUT && direction == Direction::Output;
    }
    if mission.end(node).is_some() {
        return port == END_IN && direction == Direction::Input;
    }
    false
}

/// Every wire leaving a node, from any of its ports.
pub fn outgoing<'a>(mission: &'a Mission, node: &'a str) -> impl Iterator<Item = &'a Connection> {
    (mission.connections.iter()).filter(move |connection| endpoint_node(&connection.from) == node)
}

/// Every wire leaving one port of a node.
pub fn outgoing_from_port<'a>(
    mission: &'a Mission,
    node: &'a str,
    port: &'a str,
) -> impl Iterator<Item = &'a Connection> {
    outgoing(mission, node).filter(move |connection| endpoint_port(&connection.from) == port)
}

/// Whether an endpoint is a gate's fail port: the sanctioned pushback route,
/// which may feed an input another wire already feeds.
pub fn is_gate_fail(mission: &Mission, endpoint: &str) -> bool {
    endpoint
        .split_once(':')
        .is_some_and(|(node, port)| port == GATE_FAIL && mission.gate(node).is_some())
}

/// Whether an endpoint is an End node's input, which takes any number of routes.
pub fn is_end_input(mission: &Mission, endpoint: &str) -> bool {
    split_endpoint(endpoint)
        .is_some_and(|(node, port)| port == END_IN && mission.end(node).is_some())
}

/// Every node a run reaches from `root` by following wires, `root` included.
pub fn reachable(mission: &Mission, root: &str) -> HashSet<String> {
    let mut reached = HashSet::from([root.to_owned()]);
    let mut queue = VecDeque::from([root.to_owned()]);
    while let Some(node) = queue.pop_front() {
        for connection in outgoing(mission, &node) {
            let next = endpoint_node(&connection.to);
            if !next.is_empty() && reached.insert(next.to_owned()) {
                queue.push_back(next.to_owned());
            }
        }
    }
    reached
}

/// The gate's judge chain: the steps wired from its judge port, each to the
/// next, and back to that port. From the step the judge port feeds, each step
/// either returns to the port, which completes the chain, or hands on to the
/// first step it feeds. A chain that leaves the steps, loops or never returns
/// is no chain, and the result is empty.
pub fn judge_chain<'a>(mission: &'a Mission, gate: &str) -> Vec<&'a Formation> {
    let Some(entry) = outgoing_from_port(mission, gate, GATE_JUDGE).next() else {
        return Vec::new();
    };
    let mut current = endpoint_node(&entry.to);
    let mut visited = HashSet::new();
    let mut chain = Vec::new();
    while !current.is_empty() && visited.insert(current) {
        let Some(formation) = mission.formation(current) else {
            return Vec::new();
        };
        chain.push(formation);
        let mut next = "";
        for connection in outgoing(mission, current) {
            let to = endpoint_node(&connection.to);
            if to == gate && endpoint_port(&connection.to) == GATE_JUDGE {
                return chain;
            }
            if next.is_empty() && mission.formation(to).is_some() {
                next = to;
            }
        }
        current = next;
    }
    Vec::new()
}

/// The one gate that judges a step and sends it back: only the step feeds its
/// input, and its fail route returns to the step. Otherwise says why there is
/// none, or why there are too many.
pub fn selecting_gate<'a>(mission: &'a Mission, step: &str) -> Result<&'a Gate, String> {
    let found: Vec<_> = (mission.gates.iter())
        .filter(|gate| {
            let feeds = (mission.connections.iter()).filter(|connection| {
                endpoint_node(&connection.to) == gate.id && endpoint_port(&connection.to) == GATE_IN
            });
            let (mut fed, mut other) = (false, false);
            for connection in feeds {
                if endpoint_node(&connection.from) == step {
                    fed = true;
                } else {
                    other = true;
                }
            }
            fed && !other
                && outgoing_from_port(mission, &gate.id, GATE_FAIL)
                    .any(|connection| endpoint_node(&connection.to) == step)
        })
        .collect();
    let gate = match found.as_slice() {
        [] => {
            return Err("no gate judges it and sends it back: wire a judge or human gate that only it feeds and whose fail returns to it, or set it to stop or fail".to_owned());
        }
        [gate] => *gate,
        [first, second, ..] => {
            return Err(format!(
                "{} and {} both judge it and send it back, so neither can pick: keep one, or set it to stop or fail",
                mission.node_name(&first.id),
                mission.node_name(&second.id)
            ));
        }
    };
    if !gate
        .kinds
        .iter()
        .any(|kind| kind == "formation" || kind == "human")
    {
        return Err(format!(
            "its gate {} checks with code only, which cannot compare attempts: give it a judge or a human, or set it to stop or fail",
            mission.node_name(&gate.id)
        ));
    }
    Ok(gate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::Port;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Case {
        name: String,
        formations: Vec<String>,
        connections: Vec<(String, String)>,
        chain: Vec<String>,
    }

    /// Archon's shared judge-chain cases, which its engine and cockpit both
    /// follow. Tools are gone, so a Tool there is a node that is not a step.
    #[test]
    fn judge_chains_follow_archons_cases() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../tests/fixtures/judge_chains.json")).unwrap();
        for case in cases {
            let port = |id: &str| {
                vec![Port {
                    id: id.into(),
                    ..Port::default()
                }]
            };
            let mission = Mission {
                formations: (case.formations.iter())
                    .map(|id| Formation {
                        id: id.clone(),
                        inputs: port("in"),
                        outputs: port("out"),
                        ..Formation::default()
                    })
                    .collect(),
                gates: ["gate_g", "gate_h"]
                    .map(|id| Gate {
                        id: id.into(),
                        ..Gate::default()
                    })
                    .into(),
                connections: (case.connections.iter().enumerate())
                    .map(|(i, (from, to))| Connection {
                        id: format!("edge_{i}"),
                        from: from.clone(),
                        to: to.clone(),
                    })
                    .collect(),
                ..Mission::default()
            };
            let chain: Vec<_> = judge_chain(&mission, "gate_g")
                .iter()
                .map(|f| f.id.clone())
                .collect();
            assert_eq!(chain, case.chain, "{}", case.name);
        }
    }
}
