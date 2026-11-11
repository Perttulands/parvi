//! Limit cards: each covers one step or the Input card, no target has two,
//! each knob it sets is a positive whole number, and a card that picks the
//! best attempt when spent has a gate to pick with.

use std::collections::HashMap;

use crate::finding::{Code, Finding};
use crate::graph;
use crate::limits::duration_words;
use crate::mission::Mission;
use crate::nodes::Limit;

const DEFAULT_TITLE: &str = "Limit";

/// The errors, then the warnings.
pub fn findings(mission: &Mission) -> (Vec<Finding>, Vec<Finding>) {
    let (mut errors, mut warnings) = (Vec::new(), Vec::new());
    let mut covered: HashMap<&str, &str> = HashMap::new();
    for limit in &mission.limits {
        let name = card_name(mission, &limit.id);
        let mut add =
            |message: String| errors.push(Finding::new(Code::InvalidLimit, &limit.id, message));
        let target = limit.target.as_str();
        if target.trim().is_empty() {
            add(format!(
                "{name} is wired to nothing: wire it to a step, or to the Input card for the whole mission"
            ));
        } else if !covers_step_or_input_card(mission, target) {
            add(format!(
                "{name} covers {target}, which is not a step or the Input card: wire it to a step, or to the Input card for the whole mission"
            ));
        } else if let Some(first) = covered.get(target) {
            add(format!(
                "{} has two Limit cards, {} and {}: keep one",
                mission.node_name(target),
                mission.node_name(first),
                mission.node_name(&limit.id)
            ));
        } else {
            covered.insert(target, &limit.id);
        }
        for (key, value, what) in [
            (
                "rounds",
                limit.rounds,
                "rounds must be a positive whole number",
            ),
            (
                "seconds",
                limit.seconds,
                "time must be a positive whole number of seconds",
            ),
        ] {
            if let Some(value) = value.filter(|value| *value <= 0) {
                add(format!("{name} holds {key} = {value}: {what}"));
            }
        }
        match (limit.warn_seconds, limit.seconds) {
            (None, _) => {}
            (Some(warn), _) if warn <= 0 => add(format!(
                "{name} holds warnSeconds = {warn}: the warning must be a positive whole number of seconds"
            )),
            (Some(warn), None) => add(format!(
                "{name} warns with {} left but sets no time: give it time, or clear the warning",
                duration_words(warn)
            )),
            (Some(warn), Some(seconds)) if seconds > 0 && warn >= seconds => add(format!(
                "{name} warns with {} left of {}, before any work: warn with less time left",
                duration_words(warn),
                duration_words(seconds)
            )),
            _ => {}
        }
        if let Some(tokens) = limit.tokens.filter(|tokens| *tokens <= 0) {
            add(format!(
                "{name} holds tokens = {tokens}: tokens must be a positive whole number"
            ));
        }
        if let Some(problem) = when_spent_problem(mission, limit, &name) {
            add(problem);
        }
        if limit.rounds.is_none() && limit.seconds.is_none() && limit.tokens.is_none() {
            warnings.push(Finding::new(
                Code::EmptyLimit,
                &limit.id,
                format!("{name} sets no limit: give it rounds, time or tokens, or delete it"),
            ));
        }
    }
    (errors, warnings)
}

fn covers_step_or_input_card(mission: &Mission, target: &str) -> bool {
    mission.formation(target).is_some() || mission.has_input_card(target)
}

/// "Limit Beads cap", without repeating a default-titled card's kind.
fn card_name(mission: &Mission, id: &str) -> String {
    match mission.node_name(id) {
        DEFAULT_TITLE => DEFAULT_TITLE.to_owned(),
        name => format!("Limit {name}"),
    }
}

/// Why a card cannot do what its whenSpent asks: best needs a step whose
/// selecting gate a judge or a human decides, so someone can pick.
fn when_spent_problem(mission: &Mission, limit: &Limit, name: &str) -> Option<String> {
    match limit.when_spent.trim() {
        "" | "stop" | "fail" => return None,
        "best" => {}
        _ => {
            return Some(format!(
                "{name} holds whenSpent = {:?}: set it to stop, best or fail",
                limit.when_spent
            ));
        }
    }
    if mission.has_input_card(&limit.target) {
        return Some(format!(
            "{name} picks the best attempt when spent, but it covers the whole mission: only a step's card can pick; set it to stop or fail"
        ));
    }
    mission.formation(&limit.target)?;
    let problem = graph::selecting_gate(mission, &limit.target).err()?;
    Some(format!(
        "{name} picks the best attempt of {} when spent, but {problem}",
        mission.node_name(&limit.target)
    ))
}
