//! Port kinds and mission outputs: each output is declared well, bound to
//! one step output port, and carries that port's kind.

use std::collections::HashSet;

use super::inputs::valid_name;
use super::possessive;
use crate::finding::{Code, Finding};
use crate::graph::split_endpoint;
use crate::mission::Mission;
use crate::nodes::{KIND_FILE, KIND_FOLDER, KIND_TEXT, MissionOutput, Port};

const KINDS: [&str; 3] = [KIND_TEXT, KIND_FILE, KIND_FOLDER];

/// Only output ports carry a kind, and it is text, file or folder.
pub fn port_kind_findings(mission: &Mission) -> Vec<Finding> {
    let mut findings = Vec::new();
    for formation in &mission.formations {
        let owner = possessive(mission.node_name(&formation.id));
        for port in formation
            .inputs
            .iter()
            .filter(|port| port.kind() != KIND_TEXT)
        {
            findings.push(Finding::new(
                Code::InvalidPortKind,
                &formation.id,
                format!(
                    "{owner} input {} has kind {:?}; only output ports carry a kind",
                    port.id, port.kind
                ),
            ));
        }
        for port in formation
            .outputs
            .iter()
            .filter(|port| !KINDS.contains(&port.kind()))
        {
            findings.push(Finding::new(
                Code::InvalidPortKind,
                &formation.id,
                format!(
                    "{owner} output {} has kind {:?}; use text, file or folder",
                    port.id, port.kind
                ),
            ));
        }
    }
    findings
}

/// Findings are located on the Input card that declares the output.
pub fn mission_output_findings(mission: &Mission) -> Vec<Finding> {
    let mut findings = Vec::new();
    for card in &mission.input_cards {
        let mut seen = HashSet::new();
        for output in &card.outputs {
            let mut add = |code, message| findings.push(Finding::new(code, &card.id, message));
            let problem = declaration_problem(output, &seen);
            seen.insert(output.name.as_str());
            if let Some(problem) = problem {
                add(Code::InvalidMissionOutput, problem);
                continue;
            }
            let name = &output.name;
            if output.from.is_empty() {
                add(
                    Code::UnboundMissionOutput,
                    format!(
                        "output {name} is bound to no step output; set its from to <step-id>:<port-id>"
                    ),
                );
                continue;
            }
            let Some((step, port)) = bound_port(mission, &output.from) else {
                add(
                    Code::UnboundMissionOutput,
                    format!(
                        "output {name} is bound to {}, which is not a step output port",
                        output.from
                    ),
                );
                continue;
            };
            let kind = output_kind(output);
            if port.kind() != kind {
                add(
                    Code::MissionOutputKindMismatch,
                    format!(
                        "output {name} is {kind}, but {} output {} carries {}",
                        possessive(mission.node_name(step)),
                        port.name(),
                        port.kind()
                    ),
                );
            }
        }
    }
    findings
}

fn output_kind(output: &MissionOutput) -> &str {
    if output.kind.is_empty() {
        KIND_TEXT
    } else {
        &output.kind
    }
}

fn declaration_problem(output: &MissionOutput, seen: &HashSet<&str>) -> Option<String> {
    let (name, kind) = (&output.name, output_kind(output));
    if !valid_name(name) {
        return Some(format!(
            "output name {name:?} must start with a lowercase letter and hold only lowercase letters, digits and underscores, such as report or site_dir"
        ));
    }
    if !KINDS.contains(&kind) {
        return Some(format!(
            "output {name} has kind {kind:?}; use text, file or folder"
        ));
    }
    if seen.contains(name.as_str()) {
        return Some(format!("the mission already has an output named {name}"));
    }
    if !output.from.is_empty() && split_endpoint(&output.from).is_none() {
        return Some(format!(
            "output {name} has from {:?}; name a step output port as <step-id>:<port-id>",
            output.from
        ));
    }
    None
}

/// The step and output port a mission output's `from` names.
fn bound_port<'a>(mission: &'a Mission, from: &str) -> Option<(&'a str, &'a Port)> {
    let (step, port) = split_endpoint(from)?;
    let formation = mission.formation(step)?;
    let port = formation
        .outputs
        .iter()
        .find(|candidate| candidate.id == port)?;
    Some((&formation.id, port))
}
