//! Words for the routes a human gate may take.
use super::flow::Target;

/// What an answer does, in the operator's words: the line under the
/// answer and the button. `alone` says nothing else in the run is open, so
/// a route that only ends paths ends the run (Archon's `gateRouteWords`).
pub fn route(targets: &[Target], answer: &str, alone: bool) -> (String, String) {
    let ends: Vec<&str> = targets
        .iter()
        .filter_map(|t| match t {
            Target::End { outcome, .. } => Some(outcome.as_str()),
            _ => None,
        })
        .collect();
    let steps: Vec<String> = targets
        .iter()
        .filter_map(|t| match t {
            Target::Step { number, title, .. } => Some(match number {
                Some(n) => format!("{n} {title}"),
                None => title.clone(),
            }),
            _ => None,
        })
        .collect();
    if steps.is_empty() && ends.is_empty() {
        return (format!("{answer}: this route is not wired."), answer.into());
    }
    if steps.is_empty() {
        let ended = ends
            .iter()
            .map(|o| format!("this path ends ({o})"))
            .collect::<Vec<_>>()
            .join(" and ");
        let fails = ends.contains(&"rejected");
        return match (alone, fails) {
            (true, true) => (
                format!("{answer}: {ended}, and with nothing else to run, the run fails."),
                format!("{answer} and fail the run"),
            ),
            (true, false) => (
                format!("{answer}: {ended}, and with nothing else to run, the run succeeds."),
                format!("{answer} and end the run"),
            ),
            (false, true) => (
                format!("{answer}: {ended}, so the run fails once its other open work ends."),
                answer.into(),
            ),
            (false, false) => (
                format!("{answer}: {ended}; the run goes on with its other work."),
                answer.into(),
            ),
        };
    }
    let back = targets
        .iter()
        .any(|t| matches!(t, Target::Step { back: true, .. }));
    let how = if back { "goes back to" } else { "goes on to" };
    (
        format!("{answer}: the run {how} {}.", steps.join(", ")),
        answer.into(),
    )
}

/// Names the card whose output the gate is considering.
pub fn source(mission: &parvi_missions_contract::Mission, id: &str) -> String {
    let title = mission.node_title(id).trim();
    if title.is_empty() {
        "upstream step".into()
    } else {
        title.into()
    }
}
/// Names candidates and the judge's recommendation in a pick request.
pub fn picking_words(pick: &serde_json::Value, goes: &str) -> Vec<String> {
    let attempts = pick
        .get("candidates")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_i64())
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mut words = vec![format!(
        "The step's limit is spent. Its gate judged attempts {attempts}; take the best one."
    )];
    if let Some(rec) = pick.get("recommendation")
        && let Some(attempt) = rec.get("attempt").and_then(|v| v.as_i64())
    {
        let why = rec
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        words.push(if why.is_empty() {
            format!("The judge recommends attempt {attempt}.")
        } else {
            format!("The judge recommends attempt {attempt}: {why}")
        });
    }
    words.push(goes.to_owned());
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_gate_quote_names_the_feeding_card_or_a_plain_fallback() {
        let mission = parvi_missions_contract::Mission {
            formations: vec![
                parvi_missions_contract::Formation {
                    id: "named".into(),
                    title: "Plan".into(),
                    ..Default::default()
                },
                parvi_missions_contract::Formation {
                    id: "untitled".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(source(&mission, "named"), "Plan");
        assert_eq!(source(&mission, "untitled"), "upstream step");
        assert_eq!(source(&mission, "missing"), "upstream step");
        assert_eq!(source(&mission, ""), "upstream step");
    }
    #[test]
    fn a_pick_request_names_its_attempts_and_the_judges_choice() {
        let pick = serde_json::json!({"candidates":[1,2], "recommendation":{"attempt":2,"reason":"tighter"}});
        assert_eq!(
            picking_words(&pick, "goes"),
            [
                "The step's limit is spent. Its gate judged attempts 1, 2; take the best one.",
                "The judge recommends attempt 2: tighter",
                "goes"
            ]
        );
    }
    #[test]
    fn a_route_says_where_an_answer_leads() {
        let end = |outcome: &str| Target::End {
            node: "e".into(),
            title: "End".into(),
            outcome: outcome.into(),
        };
        assert_eq!(
            route(&[end("done")], "Approve", true).1,
            "Approve and end the run"
        );
        assert_eq!(
            route(&[end("rejected")], "Send back", true).1,
            "Send back and fail the run"
        );
        assert_eq!(
            route(&[end("rejected")], "Send back", false).0,
            "Send back: this path ends (rejected), so the run fails once its other open work ends."
        );
        let back = Target::Step {
            node: "b".into(),
            number: Some(2),
            title: "Work".into(),
            back: true,
        };
        assert_eq!(
            route(&[back], "Send back", true).0,
            "Send back: the run goes back to 2 Work."
        );
        assert!(
            route(&[Target::Nowhere], "Approve", true)
                .0
                .contains("not wired")
        );
    }
}
