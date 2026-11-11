use parvi_mission_engine::{Harnesses, RoleStore, run_admission};
use parvi_mission_model::{Formation, Mission, Slot};
use std::collections::BTreeMap;

#[test]
fn admission_checks_inputs_and_frozen_staffing_across_the_mission() {
    let mut mission =
        Mission::parse(include_str!("parity/runs/missions/solo-lab.mission.toml")).unwrap();
    mission.formations.push(Formation {
        id: "unreachable".into(),
        kind: "solo".into(),
        slots: vec![Slot {
            id: "bad".into(),
            harness: "unknown".into(),
            effort: "medium".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    mission.formations.push(Formation {
        id: "unreachable_unstaffed".into(),
        kind: "solo".into(),
        slots: vec![Slot {
            id: "empty".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let roles = RoleStore::new(std::env::temp_dir().join("parvi-99-no-such-roles"));
    let report = run_admission(
        &mission,
        &roles,
        &Harnesses::default(),
        "inp_hello",
        false,
        &BTreeMap::new(),
    );
    assert!(
        report
            .errors
            .iter()
            .any(|f| f.code.as_str() == "missing_input" && f.node_id == "inp_hello")
    );
    assert!(
        report
            .errors
            .iter()
            .any(|f| f.code.as_str() == "invalid_slot_settings" && f.node_id == "unreachable")
    );
    assert!(
        report
            .errors
            .iter()
            .any(|f| f.code.as_str() == "unstaffed_slot" && f.node_id == "unreachable_unstaffed")
    );
}
