//! Wires and routes: every connection joins existing ports, each input has
//! one producer, every gate can route, every route leads somewhere, and the
//! Input card reaches the rest.

use std::collections::{HashMap, HashSet};

use super::possessive;
use crate::code_check::{code_check, code_check_names};
use crate::finding::{Code, Finding};
use crate::graph::{self, Direction};
use crate::mission::Mission;
use crate::nodes::{GATE_FAIL, GATE_PASS, Gate};

pub fn connection_findings(mission: &Mission) -> Vec<Finding> {
    let mut findings = Vec::new();
    for connection in &mission.connections {
        if !graph::endpoint_allows(mission, &connection.from, Direction::Output) {
            findings.push(Finding::new(
                Code::DanglingConnection,
                &connection.id,
                format!(
                    "connection {:?} has a broken 'from' endpoint {:?}: it does not reference an existing node output",
                    connection.id, connection.from
                ),
            ));
        }
        if !graph::endpoint_allows(mission, &connection.to, Direction::Input) {
            findings.push(Finding::new(
                Code::DanglingConnection,
                &connection.id,
                format!(
                    "connection {:?} has a broken 'to' endpoint {:?}: it does not reference an existing node input",
                    connection.id, connection.to
                ),
            ));
        }
    }
    // A gate's fail route into a fed input is the pushback route, and an End
    // takes any number of routes; neither counts as a second producer.
    let mut producers: HashMap<&str, &str> = HashMap::new();
    for connection in &mission.connections {
        if graph::is_gate_fail(mission, &connection.from)
            || graph::is_end_input(mission, &connection.to)
        {
            continue;
        }
        if let Some(first) = producers.get(connection.to.as_str()) {
            findings.push(Finding::new(
                Code::DuplicateInputProducer,
                &connection.id,
                format!(
                    "connection {:?} is a second producer for input {:?}; it is already produced by connection {first:?}",
                    connection.id, connection.to
                ),
            ));
            continue;
        }
        producers.insert(&connection.to, &connection.id);
    }
    findings
}

pub fn gate_findings(mission: &Mission) -> Vec<Finding> {
    (mission.gates.iter())
        .filter_map(|gate| {
            let gaps = route_gaps(mission, gate);
            (!gaps.is_empty()).then(|| {
                Finding::new(
                    Code::GateNotRoutable,
                    &gate.id,
                    format!("gate {:?} needs {}", gate.id, gaps.join("; ")),
                )
            })
        })
        .collect()
}

/// What a gate still needs before a run can route through it.
fn route_gaps(mission: &Mission, gate: &Gate) -> Vec<String> {
    if gate.kinds.is_empty() {
        return vec!["a kind: code, formation or human".to_owned()];
    }
    let mut gaps = Vec::new();
    let mut seen = HashSet::new();
    for kind in &gate.kinds {
        if !seen.insert(kind.as_str()) {
            continue;
        }
        match kind.as_str() {
            "code" => gaps.extend(code_check_gap(gate)),
            "formation" if graph::judge_chain(mission, &gate.id).is_empty() => {
                gaps.push(
                    "a judge chain wired from its judge port through a formation and back"
                        .to_owned(),
                );
            }
            "formation" | "human" => {}
            other => gaps.push(format!(
                "a supported kind in place of {other:?} (code, formation or human)"
            )),
        }
    }
    gaps
}

/// Names the missing or malformed part of a gate's code check.
fn code_check_gap(gate: &Gate) -> Option<String> {
    let (check, version) = (gate.check.trim(), gate.check_version.trim());
    let registered = code_check_names().join(" or ");
    if check.is_empty() && version.is_empty() {
        return Some(format!("a code check ({registered})"));
    }
    let Some(profile) = code_check(check, version) else {
        return Some(if version.is_empty() {
            format!("a version for code check {check:?}")
        } else if check.is_empty() {
            format!("a code check for version {version:?}")
        } else {
            format!("a registered code check in place of unknown {check}@{version} ({registered})")
        });
    };
    gate.check_value.trim().is_empty().then(|| {
        format!(
            "{} for code check {}@{}",
            profile.parameter.to_lowercase(),
            profile.id,
            profile.version
        )
    })
}

/// Every step output and gate pass or fail route without a wire.
pub fn route_findings(mission: &Mission) -> Vec<Finding> {
    let wired: HashSet<&str> = mission
        .connections
        .iter()
        .map(|c| c.from.as_str())
        .collect();
    let mut findings = Vec::new();
    let mut add = |node: &str, route: String| {
        findings.push(Finding::new(
            Code::RouteLeadsNowhere,
            node,
            format!(
                "{} {route} leads nowhere: wire it to a step or an End node",
                possessive(mission.node_name(node))
            ),
        ));
    };
    for formation in &mission.formations {
        for port in &formation.outputs {
            if !wired.contains(format!("{}:{}", formation.id, port.id).as_str()) {
                let route = if formation.outputs.len() > 1 {
                    format!("{:?} output", port.name())
                } else {
                    "output".to_owned()
                };
                add(&formation.id, route);
            }
        }
    }
    for gate in &mission.gates {
        for port in [GATE_PASS, GATE_FAIL] {
            if !wired.contains(format!("{}:{port}", gate.id).as_str()) {
                add(&gate.id, format!("{port} route"));
            }
        }
    }
    findings
}

/// One Input card per mission, on a channel the operator can be reached by.
pub fn input_card_findings(mission: &Mission) -> Vec<Finding> {
    let mut findings = Vec::new();
    match mission.input_cards.len() {
        0 => findings.push(Finding::new(
            Code::MissionCount,
            "",
            "a mission needs its Input card to run; add one on the canvas".to_owned(),
        )),
        1 => {}
        count => {
            let cards: Vec<_> = (mission.input_cards.iter())
                .map(|card| format!("{:?} ({})", card.title, card.id))
                .collect();
            findings.push(Finding::new(
                Code::SeveralInputCards,
                "",
                format!(
                    "mission {slug:?} holds {count} Input cards, {}; a mission has one, so no run can start from it. \
                     Split it: copy {slug}.mission.toml beside itself under a new slug and give the copy a new id, slug and title, \
                     then delete from each file the Input cards, and the steps only they reach, that belong to the other",
                    cards.join(", "),
                    slug = mission.slug,
                ),
            ));
        }
    }
    for card in &mission.input_cards {
        if !matches!(card.human_channel.trim(), "" | "notify" | "session") {
            findings.push(Finding::new(
                Code::InvalidHumanChannel,
                &card.id,
                format!(
                    "Input card {:?} has human channel {:?}; set it to notify or session",
                    card.id, card.human_channel
                ),
            ));
        }
    }
    findings
}

/// An Input card with no wire out cannot start a run.
pub fn not_runnable_findings(mission: &Mission) -> Vec<Finding> {
    (mission.input_cards.iter())
        .filter(|card| graph::outgoing(mission, &card.id).next().is_none())
        .map(|card| {
            Finding::new(
                Code::MissionNotRunnable,
                &card.id,
                format!(
                    "Input card {:?} has no outgoing connection, so it cannot start a run; wire it to a first step",
                    card.id
                ),
            )
        })
        .collect()
}

/// Every step, gate and End node no path from the Input card reaches. A
/// judge is reached through its gate's judge port.
pub fn unreachable_findings(mission: &Mission) -> Vec<Finding> {
    if mission.input_cards.is_empty() {
        return Vec::new();
    }
    let reached: HashSet<String> = (mission.input_cards.iter())
        .flat_map(|card| graph::reachable(mission, &card.id))
        .collect();
    let nodes = (mission.formations.iter().map(|n| (&n.id, "step")))
        .chain(mission.gates.iter().map(|n| (&n.id, "gate")))
        .chain(mission.ends.iter().map(|n| (&n.id, "End node")));
    nodes
        .filter(|(id, _)| !reached.contains(id.as_str()))
        .map(|(id, kind)| {
            Finding::new(
                Code::UnreachableNode,
                id,
                format!(
                    "No path from the Input card reaches {kind} {}, so no run will get there; wire a route into it or delete it",
                    mission.node_name(id)
                ),
            )
        })
        .collect()
}
