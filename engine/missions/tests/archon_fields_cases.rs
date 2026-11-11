use parvi_mission_engine::Store;
use parvi_mission_model::{Patch, Precondition};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "parvi-97-mission-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn pre(edit: &parvi_mission_model::Edit) -> Precondition {
    Precondition {
        rev: edit.mission.rev,
        etag: edit.etag.clone(),
    }
}

// Ported from node_update_test.go: partial updates retain all other fields.
#[test]
fn input_and_formation_titles_and_hint_can_be_set_and_cleared() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("updates", "Updates", "").unwrap();
    edit = store
        .patch(
            "updates",
            pre(&edit),
            Patch::CreateInputCard {
                title: "Frame".into(),
                goal: "Original".into(),
                files: vec![],
                human_channel: "".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "updates",
            pre(&edit),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "Map".into(),
                x: 1,
                y: 2,
            },
            "",
        )
        .unwrap();
    let input = edit.mission.input_cards[0].id.clone();
    let formation = edit.mission.formations[0].id.clone();
    let layout_path = dir.0.join("layout/updates.layout.toml");
    let layout = std::fs::read(&layout_path).unwrap();
    for (title, goal, hint) in [
        (
            "Frame the goal",
            "Draft a framing",
            "Paste the sketch\nand its sources.",
        ),
        ("", "", ""),
    ] {
        edit = store
            .patch(
                "updates",
                pre(&edit),
                Patch::UpdateInputCard(parvi_mission_model::InputCardUpdate {
                    id: input.clone(),
                    title: Some(title.into()),
                    goal: Some(goal.into()),
                    input_hint: Some(hint.into()),
                    ..Default::default()
                }),
                "",
            )
            .unwrap();
        let card = &edit.mission.input_cards[0];
        assert_eq!(
            (&*card.title, &*card.goal, &*card.input_hint),
            (title, goal, hint)
        );
        assert_eq!(card.id, input);
    }
    for title in ["Map territory", "", "Map again"] {
        edit = store
            .patch(
                "updates",
                pre(&edit),
                Patch::UpdateFormation {
                    id: formation.clone(),
                    title: Some(title.into()),
                    device: None,
                },
                "",
            )
            .unwrap();
        assert_eq!(edit.mission.formations[0].title, title);
        assert_eq!(edit.mission.formations[0].id, formation);
        assert_eq!(edit.mission.formations[0].slots.len(), 1);
    }
    let noop = store
        .patch(
            "updates",
            pre(&edit),
            Patch::UpdateInputCard(parvi_mission_model::InputCardUpdate {
                id: input,
                input_hint: Some("".into()),
                ..Default::default()
            }),
            "",
        )
        .unwrap();
    assert_eq!(noop.etag, edit.etag);
    assert!(
        !std::fs::read_to_string(dir.0.join("missions/updates.mission.toml"))
            .unwrap()
            .contains("inputHint")
    );
    assert_eq!(std::fs::read(layout_path).unwrap(), layout);
}

// Ported from gate_update_test.go: invalid changes leave the original bytes intact.
#[test]
fn gate_rejects_bad_kinds_and_checks_without_mutation() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("gate", "Gate", "").unwrap();
    let edit = store
        .patch(
            "gate",
            pre(&edit),
            Patch::CreateGate {
                title: "Review".into(),
                kinds: vec!["code".into()],
                criterion: "ready".into(),
                check: "output_contains".into(),
                check_version: "1".into(),
                check_value: "ok".into(),
                files: vec![],
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let id = edit.mission.gates[0].id.clone();
    let path = dir.0.join("missions/gate.mission.toml");
    let before = std::fs::read(&path).unwrap();
    for (update, code) in [
        (
            parvi_mission_model::GateUpdate {
                id: id.clone(),
                kinds: Some(vec![]),
                ..Default::default()
            },
            "invalid_gate_kind",
        ),
        (
            parvi_mission_model::GateUpdate {
                id: id.clone(),
                kinds: Some(vec!["human".into(), "robot".into()]),
                ..Default::default()
            },
            "invalid_gate_kind",
        ),
        (
            parvi_mission_model::GateUpdate {
                id: id.clone(),
                kinds: Some(vec!["human".into()]),
                check: Some("output_contains".into()),
                ..Default::default()
            },
            "invalid_code_gate_profile",
        ),
        (
            parvi_mission_model::GateUpdate {
                id: id.clone(),
                check_version: Some("999".into()),
                ..Default::default()
            },
            "invalid_code_gate_profile",
        ),
        (
            parvi_mission_model::GateUpdate {
                id: "missing".into(),
                title: Some("x".into()),
                ..Default::default()
            },
            "not_found",
        ),
    ] {
        assert_eq!(
            store
                .patch("gate", pre(&edit), Patch::UpdateGate(update), "")
                .unwrap_err()
                .code
                .as_str(),
            code
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    let changed = store
        .patch(
            "gate",
            pre(&edit),
            Patch::UpdateGate(parvi_mission_model::GateUpdate {
                id,
                title: Some("".into()),
                criterion: Some("".into()),
                check: Some("".into()),
                check_version: Some("".into()),
                check_value: Some("".into()),
                ..Default::default()
            }),
            "",
        )
        .unwrap();
    let gate = &changed.mission.gates[0];
    assert_eq!(
        (
            &*gate.title,
            &*gate.criterion,
            &*gate.check,
            &*gate.check_version,
            &*gate.check_value
        ),
        ("", "", "", "", "")
    );
    let raw = std::fs::read_to_string(path).unwrap();
    assert!(!raw.contains("checkVersion"));
    assert!(!raw.contains("checkValue"));
}

// Ported from slot_settings_test.go: a role is text; the slot states all
// executable settings, and invalid assignments must not touch the file.
#[test]
fn slot_settings_round_trip_and_invalid_assignments_write_nothing() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("seat", "Seat", "").unwrap();
    let mut edit = store
        .patch(
            "seat",
            pre(&edit),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "Worker".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let formation = edit.mission.formations[0].id.clone();
    let slot = edit.mission.formations[0].slots[0].id.clone();
    let path = dir.0.join("missions/seat.mission.toml");
    let before = std::fs::read(&path).unwrap();
    for (agent, harness, model, effort) in [
        ("", "claude-code", "opus", ""),
        ("", "", "", "low"),
        ("", "claude-code", "", "ultra"),
        ("", "hermes", "", "low"),
        ("", "claude-code", "opus 5", "low"),
        ("reviewer", "", "", ""),
        ("reviewer", "openai-codex", "", ""),
    ] {
        let patch = Patch::AssignSlot {
            formation_id: formation.clone(),
            slot_id: slot.clone(),
            agent_id: agent.into(),
            harness: harness.into(),
            model: model.into(),
            effort: effort.into(),
        };
        assert_eq!(
            store
                .patch("seat", pre(&edit), patch, "")
                .unwrap_err()
                .code
                .as_str(),
            "invalid_slot_settings"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    for (agent, harness, model, effort) in [
        ("", "claude-code", "opus", "low"),
        ("scout", "openai-codex", "", "xhigh"),
        ("", "", "", ""),
    ] {
        edit = store
            .patch(
                "seat",
                pre(&edit),
                Patch::AssignSlot {
                    formation_id: formation.clone(),
                    slot_id: slot.clone(),
                    agent_id: agent.into(),
                    harness: harness.into(),
                    model: model.into(),
                    effort: effort.into(),
                },
                "",
            )
            .unwrap();
        let actual = &store.mission_edit("seat").unwrap().mission.formations[0].slots[0];
        assert_eq!(
            (
                &*actual.agent_id,
                &*actual.harness,
                &*actual.model,
                &*actual.effort
            ),
            (agent, harness, model, effort)
        );
    }
}

// Ported from no_op_edit_test.go: repeated edits preserve bytes and revision,
// and a stale caller cannot bypass the precondition with a no-op.
#[test]
fn repeated_node_type_port_and_judge_edits_keep_bytes_and_check_etags() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("same", "Same", "").unwrap();
    edit = store
        .patch(
            "same",
            pre(&edit),
            Patch::CreateInputCard {
                title: "Input".into(),
                goal: "Goal".into(),
                files: vec![],
                human_channel: "".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "same",
            pre(&edit),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "Worker".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "same",
            pre(&edit),
            Patch::CreateGate {
                title: "Gate".into(),
                kinds: vec!["human".into()],
                criterion: "".into(),
                check: "".into(),
                check_version: "".into(),
                check_value: "".into(),
                files: vec![],
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let f = edit.mission.formations[0].clone();
    let input = edit.mission.input_cards[0].id.clone();
    let gate = edit.mission.gates[0].id.clone();
    let changes = vec![
        Patch::UpdateInputCard(parvi_mission_model::InputCardUpdate {
            id: input,
            title: Some("Framed".into()),
            goal: Some("New goal".into()),
            ..Default::default()
        }),
        Patch::UpdateFormation {
            id: f.id.clone(),
            title: Some("Renamed".into()),
            device: None,
        },
        Patch::SetFormationType {
            id: f.id.clone(),
            kind: "orchestrated".into(),
            keep_slot_id: "".into(),
            slots: None,
        },
        Patch::UpdatePort {
            formation_id: f.id.clone(),
            port_id: f.inputs[0].id.clone(),
            label: Some("Updated".into()),
            kind: None,
        },
        Patch::SetGateJudge {
            gate_id: gate.clone(),
            chain: vec![f.id],
        },
    ];
    let path = dir.0.join("missions/same.mission.toml");
    for patch in changes {
        let stale = pre(&edit);
        edit = store
            .patch("same", stale.clone(), patch.clone(), "")
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let repeated = store.patch("same", pre(&edit), patch.clone(), "").unwrap();
        assert_eq!(
            (repeated.mission.rev, repeated.etag),
            (edit.mission.rev, edit.etag.clone())
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            store
                .patch("same", stale, patch, "")
                .unwrap_err()
                .code
                .as_str(),
            "conflict"
        );
    }
}

// Port of TestS3FormationBriefRejectsUnsafeNonEmptyBeadID. Parvi's `work`
// takes any written work reference (docs/missions.md).
#[test]
fn brief_rejects_unsafe_work_references_without_writing() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("brief", "Brief", "").unwrap();
    let edit = store
        .patch(
            "brief",
            pre(&edit),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "Ship".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let id = edit.mission.formations[0].id.clone();
    let path = dir.0.join("missions/brief.mission.toml");
    let before = std::fs::read(&path).unwrap();
    for work in [
        "nohyphen",
        "Home-123",
        "chlab/123",
        "../home-pfyv",
        "home-pfyv\n",
        "bead:n:relative#home-1",
        "owner/repo#x",
    ] {
        let error = store
            .patch(
                "brief",
                pre(&edit),
                Patch::SetBrief {
                    formation_id: id.clone(),
                    brief: parvi_mission_model::Brief {
                        goal: "Ship".into(),
                        work: work.into(),
                        ..Default::default()
                    },
                },
                "",
            )
            .unwrap_err();
        assert_eq!(error.code.as_str(), "invalid_bead_id", "{work:?}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    let saved = store
        .patch(
            "brief",
            pre(&edit),
            Patch::SetBrief {
                formation_id: id,
                brief: parvi_mission_model::Brief {
                    goal: "Ship".into(),
                    repository: String::new(),
                    work: "srv-abc.2".into(),
                    files: vec!["C:/work/src/SessionPanel.tsx".into()],
                    links: vec!["https://example.com/spec".into()],
                },
            },
            "",
        )
        .unwrap();
    assert_eq!(
        saved.mission.formations[0].brief.as_ref().unwrap().work,
        "srv-abc.2"
    );
    let mut latest = saved;
    for work in ["gh:example/project#180", "bead:n1:/srv#srv-abc.2"] {
        latest = store
            .patch(
                "brief",
                pre(&latest),
                Patch::SetBrief {
                    formation_id: latest.mission.formations[0].id.clone(),
                    brief: parvi_mission_model::Brief {
                        goal: "Ship".into(),
                        work: work.into(),
                        ..Default::default()
                    },
                },
                "",
            )
            .unwrap();
    }
}

// Ports the S3 slot and controller cases: roles may repeat, seat settings do not.
#[test]
fn slots_share_a_role_but_keep_settings_and_exactly_one_controller() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("team", "Team", "").unwrap();
    let mut edit = store
        .patch(
            "team",
            pre(&edit),
            Patch::CreateFormation {
                kind: "orchestrated".into(),
                title: "Team".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let team = edit.mission.formations[0].clone();
    for (slot, harness, model, effort) in [
        (&team.slots[0], "openai-codex", "", "medium"),
        (&team.slots[1], "claude-code", "opus", "low"),
    ] {
        edit = store
            .patch(
                "team",
                pre(&edit),
                Patch::AssignSlot {
                    formation_id: team.id.clone(),
                    slot_id: slot.id.clone(),
                    agent_id: "builder".into(),
                    harness: harness.into(),
                    model: model.into(),
                    effort: effort.into(),
                },
                "",
            )
            .unwrap();
    }
    edit = store
        .patch(
            "team",
            pre(&edit),
            Patch::MakeController {
                formation_id: team.id,
                slot_id: team.slots[1].id.clone(),
            },
            "",
        )
        .unwrap();
    let slots = &edit.mission.formations[0].slots;
    assert_eq!(slots[0].agent_id, "builder");
    assert_eq!(slots[1].agent_id, "builder");
    assert_eq!(
        (&*slots[0].harness, &*slots[0].effort),
        ("openai-codex", "medium")
    );
    assert_eq!(
        (&*slots[1].harness, &*slots[1].model, &*slots[1].effort),
        ("claude-code", "opus", "low")
    );
    assert_eq!(slots.iter().filter(|slot| slot.controller).count(), 1);
    assert!(slots[1].controller);
    let raw = std::fs::read_to_string(dir.0.join("missions/team.mission.toml")).unwrap();
    for forbidden in ["sessionName", "sessionStem", "tmux"] {
        assert!(!raw.contains(forbidden));
    }
}
