//! Node ids End nodes and Limit cards share with other nodes, and End
//! outcomes.

use std::collections::HashMap;

use parvi_mission_model::{Code, DONE, Finding, Mission, REJECTED};

pub fn findings(mission: &Mission) -> Vec<Finding> {
    let mut seen: HashMap<&str, &str> = HashMap::new();
    let cards = mission
        .input_cards
        .iter()
        .map(|n| (n.id.as_str(), "Mission"));
    let formations = mission
        .formations
        .iter()
        .map(|n| (n.id.as_str(), "Formation"));
    let gates = mission.gates.iter().map(|n| (n.id.as_str(), "Gate"));
    seen.extend(cards.chain(formations).chain(gates));

    let mut findings = Vec::new();
    for end in &mission.ends {
        findings.extend(claim(&mut seen, &end.id, "End", "End node"));
        let outcome = end.outcome.trim();
        if end.outcome.is_empty() || !(outcome.is_empty() || outcome == DONE || outcome == REJECTED)
        {
            findings.push(Finding::new(
                Code::InvalidEnd,
                &end.id,
                format!(
                    "End node {} has outcome {:?}; set it to done or rejected",
                    mission.node_name(&end.id),
                    end.outcome
                ),
            ));
        }
    }
    for limit in &mission.limits {
        findings.extend(claim(&mut seen, &limit.id, "Limit", "Limit card"));
    }
    findings
}

/// Claims a node id for an End node or Limit card, or says whose it is.
fn claim<'a>(
    seen: &mut HashMap<&'a str, &'static str>,
    id: &'a str,
    kind: &'static str,
    noun: &str,
) -> Option<Finding> {
    if id.is_empty() {
        return None;
    }
    if let Some(first) = seen.get(id) {
        return Some(Finding::new(
            Code::DuplicateNodeId,
            id,
            format!("{noun} id {id:?} duplicates an existing {first} node id"),
        ));
    }
    seen.insert(id, kind);
    None
}
