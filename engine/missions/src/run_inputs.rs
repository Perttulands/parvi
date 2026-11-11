//! Admission and rendering of values supplied to a mission run.

use parvi_mission_model::{Code, Finding, Mission, MissionInput, RunInput};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

pub fn mission_run_inputs(mission: &Mission) -> Vec<MissionInput> {
    if let Some(card) = mission.input_card()
        && !card.inputs.is_empty()
    {
        return card
            .inputs
            .iter()
            .cloned()
            .map(|mut input| {
                if input.kind.is_empty() {
                    input.kind = "text".into();
                }
                input
            })
            .collect();
    }
    vec![MissionInput {
        name: "brief".into(),
        kind: "text".into(),
        required: true,
        description: mission
            .input_card()
            .map(|card| card.input_hint.clone())
            .unwrap_or_default(),
    }]
}

pub fn input_findings(mission: &Mission, supplied: &BTreeMap<String, String>) -> Vec<Finding> {
    let declared = mission_run_inputs(mission);
    let node = mission
        .input_card()
        .map(|card| card.id.as_str())
        .unwrap_or("");
    let mut findings = Vec::new();
    for input in &declared {
        let value = supplied.get(&input.name).map(|s| s.trim()).unwrap_or("");
        if value.is_empty() {
            if input.required {
                let suffix = if input.description.is_empty() {
                    String::new()
                } else {
                    format!(": {}", input.description)
                };
                findings.push(Finding::new(
                    Code::MissingInput,
                    node,
                    format!("input {} is required{}", input.name, suffix),
                ));
            }
            continue;
        }
        if input.kind == "file" || input.kind == "folder" {
            let path = Path::new(value);
            let problem = if !path.is_absolute() {
                Some(format!("{value:?} is a relative path"))
            } else {
                match path.metadata() {
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        Some(format!("{value} does not exist"))
                    }
                    Err(err) => Some(format!("{value} cannot be read: {err}")),
                    Ok(meta) if input.kind == "file" && !meta.is_file() => {
                        Some(format!("{value} is not a regular file"))
                    }
                    Ok(meta) if input.kind == "folder" && !meta.is_dir() => {
                        Some(format!("{value} is not a directory"))
                    }
                    Ok(_) => None,
                }
            };
            if let Some(problem) = problem {
                let kind = if input.kind == "file" {
                    "file"
                } else {
                    "directory"
                };
                findings.push(Finding::new(
                    Code::InvalidInput,
                    node,
                    format!(
                        "input {} must be the absolute path of an existing {kind}; {problem}",
                        input.name
                    ),
                ));
            }
        }
    }
    let names = declared
        .iter()
        .map(|input| input.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    for name in supplied
        .keys()
        .filter(|name| !declared.iter().any(|input| &input.name == *name))
    {
        findings.push(Finding::new(
            Code::UnknownInput,
            node,
            format!(
                "mission {} has no input named {name:?}; its inputs are {names}",
                mission.slug
            ),
        ));
    }
    findings.sort_by(|a, b| {
        (a.code.as_str(), &a.node_id, &a.message).cmp(&(b.code.as_str(), &b.node_id, &b.message))
    });
    findings
}

pub fn resolve_run_inputs(mission: &Mission, supplied: &BTreeMap<String, String>) -> Vec<RunInput> {
    mission_run_inputs(mission)
        .into_iter()
        .filter_map(|input| {
            let value = supplied.get(&input.name)?;
            if value.trim().is_empty() {
                return None;
            }
            Some(RunInput {
                name: input.name,
                value: if input.kind == "text" {
                    value.clone()
                } else {
                    value.trim().to_owned()
                },
                kind: input.kind,
            })
        })
        .collect()
}

pub fn render_run_inputs(mission: &Mission, inputs: &[RunInput]) -> String {
    if mission
        .input_card()
        .is_none_or(|card| card.inputs.is_empty())
        && inputs.len() == 1
        && inputs[0].name == "brief"
    {
        return inputs[0].value.clone();
    }
    inputs
        .iter()
        .map(|input| format!("{}: {}", input.name, input.value))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The placeholders that take the run's work item: Archon's `{bead}` and
/// Parvi's `{issue}`, the same value under two names.
pub const WORK_REFERENCES: [&str; 2] = ["bead", "issue"];

/// What `{bead}` and `{issue}` take for a run's canonical work reference:
/// the Bead id, as Archon gives it, or `owner/repo#N`; empty without one.
pub fn work_placeholder(work: &str) -> String {
    parvi_work_ref::parse_resolved(work)
        .map(|r| r.short())
        .unwrap_or_default()
}

/// Replace only valid input references. Inserted values are never interpreted again.
/// `{bead}` and `{issue}` take `work`, the run's work item as
/// [`work_placeholder`] gives it, unless the mission declares an input of
/// that name, which wins, including the optional-input rule when omitted.
pub fn substitute_run_inputs(
    text: &str,
    declared: &[MissionInput],
    inputs: &[RunInput],
    work: &str,
) -> String {
    let known: HashSet<&str> = declared.iter().map(|input| input.name.as_str()).collect();
    let mut result = String::new();
    let mut rest = text;
    while let Some(pos) = rest.find('{') {
        result.push_str(&rest[..pos]);
        rest = &rest[pos..];
        let escaped = rest.starts_with("{{");
        let prefix = if escaped { 2 } else { 1 };
        let after = &rest[prefix..];
        let len = after
            .bytes()
            .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
            .count();
        let name = &after[..len];
        let suffix = if escaped { "}}" } else { "}" };
        if len > 0
            && len <= 64
            && name.as_bytes()[0].is_ascii_lowercase()
            && after[len..].starts_with(suffix)
        {
            if escaped {
                result.push('{');
                result.push_str(name);
                result.push('}');
            } else if WORK_REFERENCES.contains(&name) && !known.contains(name) {
                result.push_str(work);
            } else if known.contains(name) {
                result.push_str(
                    inputs
                        .iter()
                        .find(|input| input.name == name)
                        .map(|input| input.value.as_str())
                        .unwrap_or("(not supplied)"),
                );
            } else {
                result.push('{');
                result.push_str(name);
                result.push('}');
            }
            rest = &after[len + suffix.len()..];
        } else {
            result.push('{');
            rest = &rest[1..];
        }
    }
    result.push_str(rest);
    result
}
