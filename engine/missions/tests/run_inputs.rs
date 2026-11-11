use parvi_mission_model::{InputCard, Mission, MissionInput, RunInput};
use parvi_missions::{
    input_findings, mission_run_inputs, render_run_inputs, resolve_run_inputs,
    substitute_run_inputs,
};
use std::collections::BTreeMap;

#[test]
fn admission_reports_all_bad_values_and_keeps_text_verbatim() {
    let mission = Mission {
        slug: "delivery".into(),
        input_cards: vec![InputCard {
            id: "input".into(),
            inputs: vec![
                MissionInput {
                    name: "topic".into(),
                    required: true,
                    description: "What to explore".into(),
                    ..Default::default()
                },
                MissionInput {
                    name: "file".into(),
                    kind: "file".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let supplied = BTreeMap::from([
        ("file".into(), "relative".into()),
        ("other".into(), "x".into()),
    ]);
    let findings = input_findings(&mission, &supplied);
    assert_eq!(
        findings.iter().map(|f| f.code.as_str()).collect::<Vec<_>>(),
        ["invalid_input", "missing_input", "unknown_input"]
    );
    assert!(findings.iter().all(|f| f.node_id == "input"));
    let supplied = BTreeMap::from([("topic".into(), "  hello\nworld  ".into())]);
    let resolved = resolve_run_inputs(&mission, &supplied);
    assert_eq!(
        render_run_inputs(&mission, &resolved),
        "topic:   hello\nworld  "
    );
}

#[test]
fn brief_and_substitution_follow_archon_escape_rules() {
    let mission = Mission::default();
    let declared = mission_run_inputs(&mission);
    let supplied = BTreeMap::from([("brief".into(), "  {issue}  ".into())]);
    let values = resolve_run_inputs(&mission, &supplied);
    assert_eq!(render_run_inputs(&mission, &values), "  {issue}  ");
    assert_eq!(
        substitute_run_inputs(
            "{brief} {issue} {{issue}} {{{brief}}} {unknown} {{Topic}}",
            &declared,
            &values,
            "owner/repo#1"
        ),
        "  {issue}   owner/repo#1 {issue} {{brief}} {unknown} {{Topic}}"
    );
    let declared = vec![MissionInput {
        name: "issue".into(),
        ..Default::default()
    }];
    assert_eq!(
        substitute_run_inputs("{issue}", &declared, &[], "owner/repo#1"),
        "(not supplied)"
    );
    let values = vec![RunInput {
        name: "issue".into(),
        kind: "text".into(),
        value: "chosen".into(),
    }];
    assert_eq!(
        substitute_run_inputs("{issue}", &declared, &values, "owner/repo#1"),
        "chosen"
    );
}
