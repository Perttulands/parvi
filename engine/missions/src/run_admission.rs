//! Findings for the portion of a mission reached by a new run.
use crate::graph::{endpoint_node, reachable};
use crate::{Harnesses, RoleStore, input_findings, validate};
use parvi_mission_model::{Code, Finding, Mission, Report};
use std::collections::{BTreeMap, HashSet};

/// Scope is the Input card or selected formation. Input values are checked in
/// the same report as the graph so the caller can repair all problems at once.
pub fn run_admission(
    mission: &Mission,
    roles: &RoleStore,
    harnesses: &Harnesses,
    root: &str,
    formation: bool,
    inputs: &BTreeMap<String, String>,
) -> Report {
    let selected: HashSet<String> = if formation {
        HashSet::from([root.to_owned()])
    } else {
        reachable(mission, root)
    };
    let connection_ids: HashSet<&str> = mission
        .connections
        .iter()
        .filter(|c| {
            selected.contains(endpoint_node(&c.from)) || selected.contains(endpoint_node(&c.to))
        })
        .map(|c| c.id.as_str())
        .collect();
    let in_scope = |finding: &Finding| match finding.code {
        Code::UnsupportedFormation
        | Code::UnsupportedHumanChannel
        | Code::InvalidFormationType
        | Code::InvalidFormationShape
        | Code::FormationWithoutSlots
        | Code::OrchestratedController
        | Code::SeveralInputCards
        | Code::InvalidMissionInput
        | Code::InvalidLimit
        | Code::UnstaffedSlot
        | Code::InvalidSlotSettings
        | Code::UnavailablePersona
        | Code::RetiredPersona => true,
        Code::RouteLeadsNowhere if formation => false,
        _ => {
            selected.contains(&finding.node_id) || connection_ids.contains(finding.node_id.as_str())
        }
    };
    let validation = validate(mission, roles, harnesses);
    let mut report = Report::default();
    report
        .errors
        .extend(validation.errors.into_iter().filter(&in_scope));
    for finding in validation.warnings {
        if finding.code == Code::MissionNotRunnable && finding.node_id == root && !formation {
            report.errors.push(finding);
        } else if in_scope(&finding) {
            report.warnings.push(finding);
        }
    }
    report.errors.extend(input_findings(mission, inputs));
    report.sort();
    report
}
