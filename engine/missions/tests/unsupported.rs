use parvi_mission_engine::{Harnesses, Store, run_admission, validate};
use parvi_mission_model::{Code, Mission};
use std::collections::BTreeMap;

fn gated() -> Mission {
    Mission::parse(include_str!(
        "../../../areas/missions/app/tests/fixtures/gated.mission.toml"
    ))
    .unwrap()
}
#[test]
fn reports_unsupported_capabilities_on_the_nodes_and_refuses_admission() {
    let root = parvi_testkit::fresh_dir("unsupported-validation");
    let store = Store::new(&root);
    let mut mission = gated();
    let solo = validate(&mission, &store.roles(), &Harnesses::default());
    assert!(!solo.errors.iter().any(|f| matches!(
        f.code,
        Code::UnsupportedFormation | Code::UnsupportedHumanChannel
    )));
    let formation = &mut mission.formations[0];
    formation.kind = "peer".into();
    let mut slot = formation.slots[0].clone();
    slot.id = "second".into();
    formation.slots.push(slot);
    mission.input_cards[0].human_channel = "session".into();
    let report = validate(&mission, &store.roles(), &Harnesses::default());
    assert!(
        report
            .errors
            .iter()
            .any(|f| f.code == Code::UnsupportedFormation
                && f.node_id == mission.formations[0].id
                && f.message.contains("not supported"))
    );
    assert!(
        report
            .errors
            .iter()
            .any(|f| f.code == Code::UnsupportedHumanChannel
                && f.node_id == mission.gates[0].id
                && f.message.contains("not supported"))
    );
    let admitted = run_admission(
        &mission,
        &store.roles(),
        &Harnesses::default(),
        &mission.input_cards[0].id,
        false,
        &BTreeMap::new(),
    );
    assert!(
        admitted
            .errors
            .iter()
            .any(|f| f.code == Code::UnsupportedFormation)
    );
    assert!(
        admitted
            .errors
            .iter()
            .any(|f| f.code == Code::UnsupportedHumanChannel)
    );
    mission.input_cards[0].human_channel.clear();
    mission.formations[0].kind = "orchestrated".into();
    mission.formations[0].slots[0].controller = true;
    let report = validate(&mission, &store.roles(), &Harnesses::default());
    assert!(!report.errors.iter().any(|f| matches!(
        f.code,
        Code::UnsupportedFormation | Code::UnsupportedHumanChannel | Code::OrchestratedController
    )));
    mission.input_cards[0].human_channel = "session".into();
    mission.connections.clear();
    let report = validate(&mission, &store.roles(), &Harnesses::default());
    assert!(
        !report
            .errors
            .iter()
            .any(|f| f.code == Code::UnsupportedHumanChannel),
        "an unreachable human gate does not use this channel"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_missions_can_be_repaired_without_allowing_unrelated_saves() {
    use parvi_mission_model::{InputCardUpdate, Patch, Precondition};
    let root = parvi_testkit::fresh_dir("unsupported-repair");
    let folder = root.join("missions");
    std::fs::create_dir_all(&folder).unwrap();
    let source = include_str!("../../../areas/missions/app/tests/fixtures/gated.mission.toml")
        .replace("type = \"solo\"", "type = \"peer\"")
        .replace(
            "id = \"inp_start\"",
            "id = \"inp_start\"\nhumanChannel = \"session\"",
        );
    std::fs::write(folder.join("gated.mission.toml"), source).unwrap();
    let store = Store::new(&root);
    let current = store.mission_edit("gated").unwrap();
    let findings = validate(&current.mission, &store.roles(), &Harnesses::default());
    assert!(
        findings
            .errors
            .iter()
            .any(|f| f.code == Code::UnsupportedFormation)
    );
    assert!(
        findings
            .errors
            .iter()
            .any(|f| f.code == Code::UnsupportedHumanChannel)
    );
    let pre = |edit: &parvi_mission_model::Edit| Precondition {
        rev: edit.mission.rev,
        etag: edit.etag.clone(),
    };
    let denied = store
        .patch("gated", pre(&current), Patch::Title("Unrelated".into()), "")
        .unwrap_err();
    assert_eq!(denied.code, "unsupported_formation");
    assert_eq!(store.mission_edit("gated").unwrap().etag, current.etag);
    let card = current.mission.input_cards[0].id.clone();
    let formation = current.mission.formations[0].id.clone();
    let repaired = store
        .patch(
            "gated",
            pre(&current),
            Patch::UpdateInputCard(InputCardUpdate {
                id: card,
                human_channel: Some("notify".into()),
                ..Default::default()
            }),
            "",
        )
        .unwrap();
    let repaired = store
        .patch(
            "gated",
            pre(&repaired),
            Patch::SetFormationType {
                id: formation,
                kind: "solo".into(),
                keep_slot_id: "".into(),
                slots: None,
            },
            "",
        )
        .unwrap();
    assert_eq!(repaired.mission.formations[0].kind, "solo");
    store
        .patch("gated", pre(&repaired), Patch::Title("Repaired".into()), "")
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn lab(setup: &parvi_node_kit::Setup) -> anyhow::Result<parvi_node_kit::Built> {
    parvi_missions::build_with_executor(setup, Some(Box::new(parvi_mission_engine::LabExecutor)))
}
const LAB: parvi_node_kit::Listing = parvi_node_kit::Listing {
    build: lab,
    ..parvi_missions::LISTING
};
#[tokio::test(flavor = "multi_thread")]
async fn cli_refuses_to_save_unsupported_formation_and_channel() {
    use parvi_testkit::{node::Node, sessions};
    use serde_json::json;
    let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, LAB], "")
        .await
        .unwrap();
    let folder = node.dir().join("missions/missions");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("gated.mission.toml"),
        include_str!("../../../areas/missions/app/tests/fixtures/gated.mission.toml"),
    )
    .unwrap();
    let before = std::fs::read(folder.join("gated.mission.toml")).unwrap();
    let mut output = Vec::new();
    let args = vec![
        "create".into(),
        "gated".into(),
        "--type".into(),
        "peer".into(),
        "--node".into(),
        format!("{}/", node.url()),
    ];
    assert_eq!(
        parvi_node_kit::cli::run("formation", &args, &mut output)
            .await
            .unwrap(),
        1
    );
    let error = node
        .call("formation_create", json!({"mission":"gated","type":"peer"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "unsupported_formation");
    assert!(error.message.contains("not supported"));
    let args = vec![
        "update".into(),
        "gated".into(),
        "--human-channel".into(),
        "session".into(),
        "--node".into(),
        format!("{}/", node.url()),
    ];
    assert_eq!(
        parvi_node_kit::cli::run("mission", &args, &mut output)
            .await
            .unwrap(),
        1
    );
    let error = node
        .call(
            "mission_update",
            json!({"mission":"gated","human_channel":"session"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "unsupported_human_channel");
    assert!(error.message.contains("not supported"));
    assert_eq!(
        std::fs::read(folder.join("gated.mission.toml")).unwrap(),
        before
    );
    node.stop().await;
}
