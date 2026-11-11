//! Mission inputs: declarations a hand-written file got wrong, and brief
//! references to inputs the mission does not have.

use std::collections::HashSet;

use super::possessive;
use parvi_mission_model::{
    Code, Finding, KIND_FILE, KIND_FOLDER, KIND_TEXT, Mission, MissionInput,
};

/// The input of a mission that declares none.
pub const IMPLICIT_INPUT: &str = "brief";

/// A lowercase letter, then up to 63 lowercase letters, digits or underscores.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The names of what a run supplies: the declared inputs, or `brief`.
fn run_input_names(mission: &Mission) -> Vec<&str> {
    match mission.input_card() {
        Some(card) if !card.inputs.is_empty() => {
            card.inputs.iter().map(|i| i.name.as_str()).collect()
        }
        _ => vec![IMPLICIT_INPUT],
    }
}

pub fn findings(mission: &Mission) -> Vec<Finding> {
    let mut findings = Vec::new();
    for card in &mission.input_cards {
        let mut seen = HashSet::new();
        for input in &card.inputs {
            if let Some(problem) = declaration_problem(input, &seen) {
                findings.push(Finding::new(Code::InvalidMissionInput, &card.id, problem));
            }
            seen.insert(input.name.as_str());
        }
    }
    let declared = run_input_names(mission);
    for formation in &mission.formations {
        let Some(brief) = &formation.brief else {
            continue;
        };
        let mut reported = HashSet::new();
        for name in references(&brief.goal) {
            if crate::run_inputs::WORK_REFERENCES.contains(&name)
                || declared.contains(&name)
                || !reported.insert(name)
            {
                continue;
            }
            findings.push(Finding::new(
                Code::UnknownInputReference,
                &formation.id,
                format!(
                    "{} brief references {{{name}}}, but the mission has no input named {name}; its inputs are {}",
                    possessive(mission.node_name(&formation.id)),
                    declared.join(", ")
                ),
            ));
        }
    }
    findings
}

fn declaration_problem(input: &MissionInput, seen: &HashSet<&str>) -> Option<String> {
    let kind = if input.kind.is_empty() {
        KIND_TEXT
    } else {
        input.kind.as_str()
    };
    let name = &input.name;
    if !valid_name(name) {
        return Some(format!(
            "input name {name:?} must start with a lowercase letter and hold only lowercase letters, digits and underscores, such as brief or target_repo"
        ));
    }
    if ![KIND_TEXT, KIND_FILE, KIND_FOLDER].contains(&kind) {
        return Some(format!(
            "input {name} has kind {kind:?}; use text, file or folder"
        ));
    }
    seen.contains(name.as_str())
        .then(|| format!("the mission already has an input named {name}"))
}

/// The input names a brief references as `{name}`, in order. An escaped
/// `{{name}}` stands for a literal `{name}` and is no reference.
pub fn references(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        if let Some(escaped) = after
            .strip_prefix('{')
            .and_then(|inner| braced_name(inner, "}}"))
        {
            rest = &after[1 + escaped.len() + 2..];
            continue;
        }
        if let Some(name) = braced_name(after, "}") {
            found.push(name);
            rest = &after[name.len() + 1..];
            continue;
        }
        rest = after;
    }
    found
}

/// The input name at the start of `text` when `close` follows it.
fn braced_name<'a>(text: &'a str, close: &str) -> Option<&'a str> {
    let end = text.find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))?;
    let name = &text[..end];
    (valid_name(name) && text[end..].starts_with(close)).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::references;

    #[test]
    fn references_skip_escapes_and_other_braces() {
        assert_eq!(references("Do {change} in {repo_1}."), ["change", "repo_1"]);
        assert_eq!(references("Literal {{change}} here"), Vec::<&str>::new());
        assert_eq!(references("{{{change}}}"), Vec::<&str>::new());
        assert_eq!(
            references("{ change } {Change} {1x} {a-b} {x"),
            Vec::<&str>::new()
        );
        assert_eq!(references("json {\"a\": {b}}"), ["b"]);
        assert_eq!(references("{{x}"), ["x"]);
    }
}
