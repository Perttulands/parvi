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
#[test]
fn revision_and_etag_fence_changes_and_noop() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let created = store.create_mission("work", "Work", "human:ui").unwrap();
    let noop = store
        .patch(
            "work",
            pre(&created),
            Patch::Title("Work".into()),
            "human:ui",
        )
        .unwrap();
    assert_eq!(noop.etag, created.etag);
    assert_eq!(noop.mission.rev, 1);
    let changed = store
        .patch(
            "work",
            pre(&created),
            Patch::Title("Next".into()),
            "human:ui",
        )
        .unwrap();
    assert_eq!(changed.mission.rev, 2);
    assert_eq!(
        store
            .patch(
                "work",
                pre(&created),
                Patch::Title("Stale".into()),
                "human:ui"
            )
            .unwrap_err()
            .code
            .as_str(),
        "conflict"
    );
    assert_eq!(store.read_mission("work").unwrap().title, "Next");
}
#[cfg(unix)]
#[test]
fn edit_preserves_comments_and_the_mission_symlink() {
    use std::os::unix::fs::symlink;
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let created = store.create_mission("work", "Work", "human:ui").unwrap();
    let link = dir.0.join("missions/work.mission.toml");
    let target = dir.0.join("target.toml");
    let text = std::fs::read_to_string(&link)
        .unwrap()
        .replace(
            "title = \"Work\"",
            "# Keep this comment\ntitle = \"Work\" # title comment",
        )
        .replace("rev = 1", "rev = 1 # revision comment");
    std::fs::write(&target, text).unwrap();
    std::fs::remove_file(&link).unwrap();
    symlink("../target.toml", &link).unwrap();
    let before = store.mission_edit("work").unwrap();
    assert_eq!(before.mission.id, created.mission.id);
    store
        .patch(
            "work",
            pre(&before),
            Patch::Title("Next".into()),
            "human:ui",
        )
        .unwrap();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let saved = std::fs::read_to_string(target).unwrap();
    assert!(saved.contains("# Keep this comment"));
    assert!(saved.contains("title = \"Next\" # title comment"));
    assert!(saved.contains("rev = 2 # revision comment"));
}

#[test]
fn create_nodes_with_default_shapes() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("work", "Work", "human:ui").unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateInputCard {
                title: "Input".into(),
                goal: String::new(),
                files: vec![],
                human_channel: String::new(),
                x: 10,
                y: 20,
            },
            "human:ui",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateFormation {
                x: 30,
                y: 40,
                kind: "orchestrated".into(),
                title: String::new(),
            },
            "human:ui",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateGate {
                title: "Review".into(),
                x: 50,
                y: 60,
                kinds: vec![],
                criterion: String::new(),
                check: String::new(),
                check_version: String::new(),
                check_value: String::new(),
                files: vec![],
            },
            "human:ui",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateEnd {
                title: String::new(),
                x: 70,
                y: 80,
                outcome: String::new(),
            },
            "human:ui",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[0].slots.len(), 3);
    assert_eq!(edit.mission.gates[0].kinds, ["formation"]);
    assert_eq!(edit.mission.ends[0].outcome, "done");
    let formation_id = edit.mission.formations[0].id.clone();
    let layout = store.read_layout("work").unwrap().unwrap();
    assert_eq!(layout.nodes.len(), 4);
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::DeleteFormation {
                id: formation_id.clone(),
            },
            "human:ui",
        )
        .unwrap();
    assert!(edit.mission.formations.is_empty());
    assert!(
        !store
            .read_layout("work")
            .unwrap()
            .unwrap()
            .nodes
            .iter()
            .any(|node| node.id == formation_id)
    );
}

#[test]
fn delete_archives_mission_and_layout_without_touching_notes() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("work", "Work", "human:ui").unwrap();
    let edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateEnd {
                title: String::new(),
                outcome: String::new(),
                x: 1,
                y: 2,
            },
            "human:ui",
        )
        .unwrap();
    std::fs::create_dir_all(dir.0.join("notes")).unwrap();
    std::fs::write(dir.0.join("notes/work.notes.toml"), "preserved").unwrap();
    let removed = store.delete_mission("work", pre(&edit)).unwrap();
    assert!(store.list_missions().unwrap().is_empty());
    assert!(
        dir.0
            .join(format!(
                "missions/work.mission.toml.deleted-{}",
                removed.archive_id
            ))
            .exists()
    );
    assert!(
        dir.0
            .join(format!(
                "layout/work.layout.toml.deleted-{}",
                removed.archive_id
            ))
            .exists()
    );
    assert_eq!(
        std::fs::read_to_string(dir.0.join("notes/work.notes.toml")).unwrap(),
        "preserved"
    );
}

#[test]
fn join_creates_a_new_input_and_refusals_do_not_edit() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("join", "Join", "").unwrap();
    for name in ["One", "Two", "Target"] {
        edit = store
            .patch(
                "join",
                pre(&edit),
                Patch::CreateFormation {
                    kind: "solo".into(),
                    title: name.into(),
                    x: 0,
                    y: 0,
                },
                "",
            )
            .unwrap();
    }
    let source = |i: usize| {
        format!(
            "{}:{}",
            edit.mission.formations[i].id, edit.mission.formations[i].outputs[0].id
        )
    };
    let first = source(0);
    let second = source(1);
    let to = format!(
        "{}:{}",
        edit.mission.formations[2].id, edit.mission.formations[2].inputs[0].id
    );
    edit = store
        .patch(
            "join",
            pre(&edit),
            Patch::WireConnection {
                id: String::new(),
                from: first.clone(),
                to: to.clone(),
                join_if_occupied: false,
            },
            "",
        )
        .unwrap();
    let old = edit.etag.clone();
    let error = store
        .patch(
            "join",
            pre(&edit),
            Patch::WireConnection {
                id: String::new(),
                from: second.clone(),
                to: to.clone(),
                join_if_occupied: false,
            },
            "",
        )
        .unwrap_err();
    assert_eq!(error.code.as_str(), "input_occupied");
    assert_eq!(store.mission_edit("join").unwrap().etag, old);
    edit = store
        .patch(
            "join",
            pre(&edit),
            Patch::WireConnection {
                id: String::new(),
                from: second,
                to: to.clone(),
                join_if_occupied: true,
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[2].inputs.len(), 2);
    assert_ne!(edit.mission.connections[1].to, to);
    edit = store
        .patch(
            "join",
            pre(&edit),
            Patch::UnwireConnection { from: first, to },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.connections.len(), 1);
}

#[test]
fn notes_are_author_owned_and_noops_keep_the_etag() {
    use parvi_mission_model::NotePatch;
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    store.create_mission("work", "Work", "").unwrap();
    let created = store
        .patch_notes(
            "work",
            "*",
            NotePatch::Append {
                target: "mission".into(),
                author: "human:ui".into(),
                text: "hello".into(),
            },
        )
        .unwrap();
    let id = created.notes.entries[0].id.clone();
    let stale = store
        .patch_notes(
            "work",
            "*",
            NotePatch::Delete {
                target: "mission".into(),
                author: "human:ui".into(),
                id: id.clone(),
            },
        )
        .unwrap_err();
    assert_eq!(stale.code.as_str(), "conflict");
    let wrong = store
        .patch_notes(
            "work",
            &created.etag,
            NotePatch::Delete {
                target: "mission".into(),
                author: "agent:other".into(),
                id: id.clone(),
            },
        )
        .unwrap_err();
    assert_eq!(wrong.code.as_str(), "note_author_mismatch");
    let noop = store
        .patch_notes(
            "work",
            &created.etag,
            NotePatch::Edit {
                target: "mission".into(),
                author: "human:ui".into(),
                id: id.clone(),
                text: "hello".into(),
            },
        )
        .unwrap();
    assert_eq!(noop.etag, created.etag);
    let deleted = store
        .patch_notes(
            "work",
            &noop.etag,
            NotePatch::Delete {
                target: "mission".into(),
                author: "human:ui".into(),
                id,
            },
        )
        .unwrap();
    assert!(deleted.notes.entries.is_empty());
    assert_eq!(deleted.notes.rev, 2);
}

#[test]
fn layout_changes_are_etag_checked_and_keep_other_layers() {
    use parvi_mission_model::{LayoutEdge, LayoutNode, LayoutPatch};
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    store.create_mission("work", "Work", "").unwrap();
    assert_eq!(
        store
            .patch_layout("work", "*", LayoutPatch::Edges(vec![]))
            .unwrap_err()
            .code
            .as_str(),
        "not_found"
    );
    assert_eq!(
        store
            .patch_layout("work", "stale", LayoutPatch::Nodes(vec![]))
            .unwrap_err()
            .code
            .as_str(),
        "not_found"
    );
    assert_eq!(
        store
            .patch_layout("work", "stale", LayoutPatch::Edges(vec![]))
            .unwrap_err()
            .code
            .as_str(),
        "not_found"
    );
    let nodes = store
        .patch_layout(
            "work",
            "*",
            LayoutPatch::Nodes(vec![LayoutNode {
                id: "note:mission".into(),
                x: 14,
                y: 28,
            }]),
        )
        .unwrap();
    let edges = store
        .patch_layout(
            "work",
            &nodes.etag,
            LayoutPatch::Edges(vec![LayoutEdge {
                id: "edge_a".into(),
                lane: "top".into(),
            }]),
        )
        .unwrap();
    assert_eq!(edges.layout.nodes[0].x, 14);
    assert_eq!(edges.layout.edges[0].lane, "top");
    assert_eq!(
        store
            .patch_layout("work", &nodes.etag, LayoutPatch::Nodes(vec![]))
            .unwrap_err()
            .code
            .as_str(),
        "conflict"
    );
}

#[test]
fn type_changes_require_an_explicit_choice_before_dropping_staffed_slots() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("work", "Work", "").unwrap();
    let mut edit = store
        .patch(
            "work",
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
    let formation = edit.mission.formations[0].clone();
    let mut slots = formation.slots.clone();
    for slot in &mut slots {
        slot.harness = "claude-code".into();
        slot.effort = "medium".into();
    }
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::SetFormationType {
                id: formation.id.clone(),
                kind: "orchestrated".into(),
                keep_slot_id: String::new(),
                slots: Some(slots.clone()),
            },
            "",
        )
        .unwrap();
    let error = store
        .patch(
            "work",
            pre(&edit),
            Patch::SetFormationType {
                id: formation.id.clone(),
                kind: "solo".into(),
                keep_slot_id: String::new(),
                slots: None,
            },
            "",
        )
        .unwrap_err();
    assert_eq!(error.code.as_str(), "slot_choice_required");
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::SetFormationType {
                id: formation.id,
                kind: "solo".into(),
                keep_slot_id: slots[1].id.clone(),
                slots: None,
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[0].slots[0].id, slots[1].id);
}

#[test]
fn staffing_is_validated_and_repeated_assignment_is_a_noop() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("work", "Work", "").unwrap();
    let mut edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "Agent".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let formation = &edit.mission.formations[0];
    let (formation_id, slot_id) = (formation.id.clone(), formation.slots[0].id.clone());
    let assignment = || Patch::AssignSlot {
        formation_id: formation_id.clone(),
        slot_id: slot_id.clone(),
        agent_id: "builder".into(),
        harness: "claude-code".into(),
        model: String::new(),
        effort: "medium".into(),
    };
    edit = store.patch("work", pre(&edit), assignment(), "").unwrap();
    assert_eq!(edit.mission.formations[0].slots[0].agent_id, "builder");
    let repeated = store.patch("work", pre(&edit), assignment(), "").unwrap();
    assert_eq!(repeated.etag, edit.etag);
    let bad = store
        .patch(
            "work",
            pre(&edit),
            Patch::AssignSlot {
                formation_id,
                slot_id,
                agent_id: String::new(),
                harness: "claude-code".into(),
                model: String::new(),
                effort: "ultra".into(),
            },
            "",
        )
        .unwrap_err();
    assert_eq!(bad.code.as_str(), "invalid_slot_settings");
}

#[test]
fn arrange_follows_delivery_and_puts_the_judge_under_its_gate() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
    let mission = Store::new(path).read_mission("delivery").unwrap();
    let result = parvi_mission_engine::arrange(&mission, &Default::default());
    let node = |id: &str| result.nodes.iter().find(|node| node.id == id).unwrap();
    let ids = [
        "mis_delivery",
        "fmn_plan",
        "fmn_beads",
        "gate_beads_review",
        "fmn_execution",
        "fmn_final_review",
        "end_delivered",
    ];
    for pair in ids.windows(2) {
        let (a, b) = (node(pair[0]), node(pair[1]));
        assert!(
            b.y > a.y || (b.y == a.y && b.x > a.x),
            "{} -> {}",
            pair[0],
            pair[1]
        );
    }
    assert_eq!(node("fmn_beads_reviewer").x, node("gate_beads_review").x);
    assert!(node("fmn_beads_reviewer").y > node("gate_beads_review").y);
}

#[test]
fn input_card_updates_validate_declarations_and_preserve_identity() {
    use parvi_mission_model::{InputCardUpdate, MissionInput, MissionOutput};
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("work", "Work", "").unwrap();
    let mut edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateInputCard {
                title: "Input".into(),
                goal: String::new(),
                files: vec![],
                human_channel: String::new(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let id = edit.mission.input_cards[0].id.clone();
    let update = InputCardUpdate {
        id: id.clone(),
        inputs: Some(vec![MissionInput {
            name: "change".into(),
            kind: String::new(),
            required: true,
            ..Default::default()
        }]),
        outputs: Some(vec![MissionOutput {
            name: "report".into(),
            kind: "file".into(),
            from: "step:output".into(),
            ..Default::default()
        }]),
        ..Default::default()
    };
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::UpdateInputCard(update.clone()),
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.input_cards[0].id, id);
    assert_eq!(edit.mission.input_cards[0].inputs[0].kind, "text");
    assert_eq!(
        store
            .patch("work", pre(&edit), Patch::UpdateInputCard(update), "")
            .unwrap()
            .etag,
        edit.etag
    );
    let invalid = InputCardUpdate {
        id,
        outputs: Some(vec![MissionOutput {
            name: "Bad".into(),
            ..Default::default()
        }]),
        ..Default::default()
    };
    assert_eq!(
        store
            .patch("work", pre(&edit), Patch::UpdateInputCard(invalid), "")
            .unwrap_err()
            .code
            .as_str(),
        "invalid_mission_output"
    );
}

#[test]
fn deleted_node_and_port_restore_their_identities_and_wires() {
    use parvi_mission_model::RestoredNode;
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("work", "Work", "").unwrap();
    for title in ["A", "B"] {
        edit = store
            .patch(
                "work",
                pre(&edit),
                Patch::CreateFormation {
                    kind: "solo".into(),
                    title: title.into(),
                    x: 0,
                    y: 0,
                },
                "",
            )
            .unwrap();
    }
    let first = edit.mission.formations[0].clone();
    let second = edit.mission.formations[1].clone();
    let from = format!("{}:{}", first.id, first.outputs[0].id);
    let to = format!("{}:{}", second.id, second.inputs[0].id);
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::WireConnection {
                id: String::new(),
                from: from.clone(),
                to: to.clone(),
                join_if_occupied: false,
            },
            "",
        )
        .unwrap();
    let wire = edit.mission.connections[0].clone();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::RemovePort {
                formation_id: first.id.clone(),
                port_id: first.outputs[0].id.clone(),
            },
            "",
        )
        .unwrap();
    assert!(edit.mission.connections.is_empty());
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::RestorePort {
                formation_id: first.id.clone(),
                direction: "output".into(),
                port: first.outputs[0].clone(),
                index: Some(0),
                connections: vec![wire.clone()],
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.connections[0], wire);
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::DeleteFormation {
                id: first.id.clone(),
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::RestoreNode {
                node: RestoredNode::Formation(first.clone()),
                connections: vec![wire.clone()],
                index: Some(0),
                x: 10,
                y: 20,
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[0], first);
    assert_eq!(edit.mission.connections[0], wire);
}

#[test]
fn judge_and_limit_edits_use_the_same_revision_path() {
    use parvi_mission_model::{GateUpdate, Limit, LimitUpdate};
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("work", "Work", "").unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "Judge".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateGate {
                title: "Review".into(),
                kinds: vec![],
                criterion: String::new(),
                check: String::new(),
                check_version: String::new(),
                check_value: String::new(),
                files: vec![],
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let judge = edit.mission.formations[0].id.clone();
    let gate = edit.mission.gates[0].id.clone();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::SetGateJudge {
                gate_id: gate.clone(),
                chain: vec![judge.clone()],
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.connections.len(), 2);
    let repeated = store
        .patch(
            "work",
            pre(&edit),
            Patch::SetGateJudge {
                gate_id: gate.clone(),
                chain: vec![judge.clone()],
            },
            "",
        )
        .unwrap();
    assert_eq!(repeated.etag, edit.etag);
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::DetachGateJudge {
                gate_id: gate.clone(),
            },
            "",
        )
        .unwrap();
    assert!(edit.mission.connections.is_empty());
    assert_eq!(edit.mission.gates[0].kinds, ["human"]);
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::UpdateGate(GateUpdate {
                id: gate,
                criterion: Some("Review it".into()),
                ..Default::default()
            }),
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateLimit {
                limit: Limit {
                    target: judge,
                    rounds: Some(3),
                    ..Default::default()
                },
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let limit = edit.mission.limits[0].id.clone();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::UpdateLimit(LimitUpdate {
                id: limit,
                rounds: Some(0),
                ..Default::default()
            }),
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.limits[0].rounds, None);
}

#[test]
fn concurrent_mission_edits_from_distinct_stores_do_not_both_win() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let original = store.create_mission("work", "Work", "").unwrap();
    let results =
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for title in ["One", "Two"] {
                let store = Store::new(&dir.0);
                let precondition = pre(&original);
                handles.push(scope.spawn(move || {
                    store.patch("work", precondition, Patch::Title(title.into()), "")
                }));
            }
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| r.as_ref().is_err_and(|e| e.code.as_str() == "conflict"))
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn node_create_does_not_publish_mission_if_layout_cannot_be_staged() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let before = store.create_mission("work", "Work", "").unwrap();
    let layout = dir.0.join("layout");
    std::fs::create_dir(&layout).unwrap();
    std::fs::set_permissions(&layout, std::fs::Permissions::from_mode(0o500)).unwrap();
    let probe = layout.join("write-probe");
    if std::fs::write(&probe, b"probe").is_ok() {
        std::fs::remove_file(probe).unwrap();
        std::fs::set_permissions(&layout, std::fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let result = store.patch(
        "work",
        pre(&before),
        Patch::CreateEnd {
            title: String::new(),
            outcome: String::new(),
            x: 0,
            y: 0,
        },
        "",
    );
    std::fs::set_permissions(&layout, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(store.mission_edit("work").unwrap().etag, before.etag);
}

#[test]
fn removing_code_kind_clears_check_and_relative_files_are_refused() {
    use parvi_mission_model::GateUpdate;
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("work", "Work", "").unwrap();
    let mut edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateGate {
                title: "Review".into(),
                kinds: vec!["code".into()],
                criterion: String::new(),
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
    let gate = edit.mission.gates[0].id.clone();
    let invalid = store
        .patch(
            "work",
            pre(&edit),
            Patch::UpdateGate(GateUpdate {
                id: gate.clone(),
                files: Some(vec!["relative.md".into()]),
                ..Default::default()
            }),
            "",
        )
        .unwrap_err();
    assert_eq!(invalid.code.as_str(), "relative_file_reference");
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::UpdateGate(GateUpdate {
                id: gate,
                kinds: Some(vec!["human".into()]),
                ..Default::default()
            }),
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.gates[0].check, "");
    assert_eq!(edit.mission.gates[0].check_version, "");
}

#[test]
fn free_layout_position_accounts_for_saved_and_fallback_nodes() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("work", "Work", "").unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "A".into(),
                x: 112,
                y: 112,
            },
            "",
        )
        .unwrap();
    let free = store.find_free_layout_position("work", 112, 112).unwrap();
    assert_eq!((free.x, free.y), (448, 112));
    assert_eq!(edit.mission.formations.len(), 1);
}

#[test]
fn rewire_join_undo_removes_only_its_extra_port_and_keeps_wire_id() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("work", "Work", "").unwrap();
    for title in ["A", "B", "Target", "Other"] {
        edit = store
            .patch(
                "work",
                pre(&edit),
                Patch::CreateFormation {
                    kind: "solo".into(),
                    title: title.into(),
                    x: 0,
                    y: 0,
                },
                "",
            )
            .unwrap();
    }
    let endpoint = |n: usize, out: bool| {
        let f = &edit.mission.formations[n];
        format!(
            "{}:{}",
            f.id,
            if out {
                &f.outputs[0].id
            } else {
                &f.inputs[0].id
            }
        )
    };
    let (a, b, target, other) = (
        endpoint(0, true),
        endpoint(1, true),
        endpoint(2, false),
        endpoint(3, false),
    );
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::WireConnection {
                id: String::new(),
                from: a,
                to: target.clone(),
                join_if_occupied: false,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::WireConnection {
                id: String::new(),
                from: b.clone(),
                to: target,
                join_if_occupied: true,
            },
            "",
        )
        .unwrap();
    let joined = edit.mission.connections[1].clone();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::RewireConnection {
                from: b,
                previous_to: joined.to.clone(),
                to: other,
                join_if_occupied: false,
                remove_previous_input: true,
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[2].inputs.len(), 1);
    assert!(edit.mission.connections.iter().any(|c| c.id == joined.id));
}

#[test]
fn restored_node_allocates_new_wire_id_when_original_is_taken() {
    use parvi_mission_model::RestoredNode;
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("work", "Work", "").unwrap();
    for title in ["A", "B", "Target", "Other"] {
        edit = store
            .patch(
                "work",
                pre(&edit),
                Patch::CreateFormation {
                    kind: "solo".into(),
                    title: title.into(),
                    x: 0,
                    y: 0,
                },
                "",
            )
            .unwrap();
    }
    let b = edit.mission.formations[1].clone();
    let output = |n: usize| {
        format!(
            "{}:{}",
            edit.mission.formations[n].id, edit.mission.formations[n].outputs[0].id
        )
    };
    let input = |n: usize| {
        format!(
            "{}:{}",
            edit.mission.formations[n].id, edit.mission.formations[n].inputs[0].id
        )
    };
    let (a_out, b_out, target_in, other_in) = (output(0), output(1), input(2), input(3));
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::WireConnection {
                id: "edge_original".into(),
                from: b_out,
                to: target_in,
                join_if_occupied: false,
            },
            "",
        )
        .unwrap();
    let original = edit.mission.connections[0].clone();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::DeleteFormation { id: b.id.clone() },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::WireConnection {
                id: "edge_original".into(),
                from: a_out,
                to: other_in,
                join_if_occupied: false,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::RestoreNode {
                node: RestoredNode::Formation(b.clone()),
                connections: vec![original.clone()],
                index: Some(1),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let restored = edit
        .mission
        .connections
        .iter()
        .find(|c| c.from == original.from && c.to == original.to)
        .unwrap();
    assert_ne!(restored.id, original.id);
    assert!(edit.mission.connections.iter().any(|c| c.id == original.id));
}

#[test]
fn placement_refuses_a_full_bounded_search_and_a_foreign_layout() {
    use parvi_mission_engine::free_layout_position;
    use parvi_mission_model::{Formation, Layout, LayoutNode, Mission};
    let mut mission = Mission {
        id: "msn_work".into(),
        ..Default::default()
    };
    let mut layout = Layout {
        schema: 1,
        mission_id: mission.id.clone(),
        ..Default::default()
    };
    let (mut x, mut y) = (112, 112);
    for index in 0..24 {
        let id = format!("f{index}");
        mission.formations.push(Formation {
            id: id.clone(),
            ..Default::default()
        });
        layout.nodes.push(LayoutNode { id, x, y });
        x += 336;
        if x > 1900 {
            x = 112;
            y += 336;
        }
    }
    assert_eq!(
        free_layout_position(&mission, Some(&layout), 112, 112)
            .unwrap_err()
            .code
            .as_str(),
        "conflict"
    );
    layout.mission_id = "other".into();
    assert_eq!(
        free_layout_position(&mission, Some(&layout), 0, 0)
            .unwrap_err()
            .code
            .as_str(),
        "conflict"
    );
}

#[test]
fn each_node_kind_round_trips_through_delete_and_restore() {
    use parvi_mission_model::{Limit, RestoredNode};
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("work", "Work", "").unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateInputCard {
                title: "Input".into(),
                goal: "Do it".into(),
                files: vec![],
                human_channel: "session".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateGate {
                title: "Check".into(),
                kinds: vec!["human".into()],
                criterion: "Confirm".into(),
                check: String::new(),
                check_version: String::new(),
                check_value: String::new(),
                files: vec![],
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateEnd {
                title: "Done".into(),
                outcome: "done".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateLimit {
                limit: Limit {
                    title: "Cap".into(),
                    target: edit.mission.input_cards[0].id.clone(),
                    rounds: Some(3),
                    ..Default::default()
                },
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let nodes = [
        RestoredNode::InputCard(edit.mission.input_cards[0].clone()),
        RestoredNode::Gate(edit.mission.gates[0].clone()),
        RestoredNode::End(edit.mission.ends[0].clone()),
        RestoredNode::Limit(edit.mission.limits[0].clone()),
    ];
    for node in nodes {
        let delete = match &node {
            RestoredNode::InputCard(n) => Patch::DeleteInputCard { id: n.id.clone() },
            RestoredNode::Gate(n) => Patch::DeleteGate { id: n.id.clone() },
            RestoredNode::End(n) => Patch::DeleteEnd { id: n.id.clone() },
            RestoredNode::Limit(n) => Patch::DeleteLimit { id: n.id.clone() },
            RestoredNode::Formation(_) => unreachable!(),
        };
        let before = edit.mission.clone();
        edit = store.patch("work", pre(&edit), delete, "").unwrap();
        edit = store
            .patch(
                "work",
                pre(&edit),
                Patch::RestoreNode {
                    node,
                    connections: vec![],
                    index: Some(0),
                    x: 0,
                    y: 0,
                },
                "",
            )
            .unwrap();
        assert_eq!(edit.mission.input_cards, before.input_cards);
        assert_eq!(edit.mission.gates, before.gates);
        assert_eq!(edit.mission.ends, before.ends);
        assert_eq!(edit.mission.limits, before.limits);
    }
}

#[test]
fn invalid_restore_leaves_mission_and_layout_untouched() {
    use parvi_mission_model::{Formation, RestoredNode, Slot};
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("work", "Work", "").unwrap();
    let bad = Formation {
        id: "fmn_bad".into(),
        kind: "peer".into(),
        slots: vec![Slot {
            id: "only".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let err = store
        .patch(
            "work",
            pre(&edit),
            Patch::RestoreNode {
                node: RestoredNode::Formation(bad),
                connections: vec![],
                index: None,
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap_err();
    assert_eq!(err.code.as_str(), "invalid_node_restore");
    assert_eq!(store.mission_edit("work").unwrap().etag, edit.etag);
    assert!(store.read_layout("work").unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn new_mission_artifacts_are_shared_group_writable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new();
    Store::new(&dir.0)
        .create_mission("work", "Work", "")
        .unwrap();
    let missions = dir.0.join("missions");
    assert_eq!(
        std::fs::metadata(missions.join("work.mission.toml"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o660
    );
    assert_eq!(
        std::fs::metadata(missions).unwrap().permissions().mode() & 0o7777,
        0o2770
    );
}

#[test]
fn gate_fail_feedback_can_share_an_occupied_input_without_a_join() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("work", "Work", "").unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateInputCard {
                title: "Input".into(),
                goal: String::new(),
                files: vec![],
                human_channel: String::new(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
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
            "work",
            pre(&edit),
            Patch::CreateGate {
                title: "Review".into(),
                kinds: vec!["human".into()],
                criterion: String::new(),
                check: String::new(),
                check_version: String::new(),
                check_value: String::new(),
                files: vec![],
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let input = format!("{}:out", edit.mission.input_cards[0].id);
    let target = format!(
        "{}:{}",
        edit.mission.formations[0].id, edit.mission.formations[0].inputs[0].id
    );
    let fail = format!("{}:fail", edit.mission.gates[0].id);
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::WireConnection {
                id: String::new(),
                from: input,
                to: target.clone(),
                join_if_occupied: false,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::WireConnection {
                id: String::new(),
                from: fail,
                to: target.clone(),
                join_if_occupied: true,
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[0].inputs.len(), 1);
    assert!(
        edit.mission
            .connections
            .iter()
            .all(|wire| wire.to == target)
    );
}

#[test]
fn repeated_brief_gate_and_controller_edits_save_nothing() {
    use parvi_mission_model::{Brief, GateUpdate};
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("work", "Work", "").unwrap();
    edit = store
        .patch(
            "work",
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
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateGate {
                title: "Review".into(),
                kinds: vec!["code".into()],
                criterion: "check".into(),
                check: "output_contains".into(),
                check_version: "1".into(),
                check_value: "OK".into(),
                files: vec![],
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let formation = edit.mission.formations[0].id.clone();
    let slot = edit.mission.formations[0].slots[1].id.clone();
    let gate = edit.mission.gates[0].id.clone();
    let patches = [
        Patch::SetBrief {
            formation_id: formation.clone(),
            brief: Brief {
                goal: "Do it".into(),
                repository: String::new(),
                work: "archon-demo".into(),
                files: vec![],
                links: vec![],
            },
        },
        Patch::MakeController {
            formation_id: formation,
            slot_id: slot,
        },
        Patch::UpdateGate(GateUpdate {
            id: gate,
            criterion: Some("approved".into()),
            ..Default::default()
        }),
    ];
    for patch in patches {
        edit = store.patch("work", pre(&edit), patch.clone(), "").unwrap();
        let unchanged = store.patch("work", pre(&edit), patch, "").unwrap();
        assert_eq!(
            (unchanged.mission.rev, &unchanged.etag),
            (edit.mission.rev, &edit.etag)
        );
    }
}

#[test]
fn notes_refuse_missing_targets_large_entries_and_old_mission_ids() {
    use parvi_mission_model::NotePatch;
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let original = store.create_mission("work", "First", "").unwrap();
    let patch = |target: &str, text: String| NotePatch::Append {
        target: target.into(),
        author: "human:ui".into(),
        text,
    };
    assert_eq!(
        store
            .patch_notes("work", "*", patch("missing", "x".into()))
            .unwrap_err()
            .code
            .as_str(),
        "not_found"
    );
    assert_eq!(
        store
            .patch_notes("work", "*", patch("mission", "x".repeat(64 * 1024 + 1)))
            .unwrap_err()
            .code
            .as_str(),
        "note_too_large"
    );
    let old_notes = store
        .patch_notes("work", "*", patch("mission", "Old".into()))
        .unwrap();
    store.delete_mission("work", pre(&original)).unwrap();
    store.create_mission("work", "Second", "").unwrap();
    assert!(store.notes_edit("work").unwrap().notes.entries.is_empty());
    assert_eq!(
        store
            .patch_notes("work", &old_notes.etag, patch("mission", "Stale".into()))
            .unwrap_err()
            .code
            .as_str(),
        "conflict"
    );
    let new_notes = store
        .patch_notes("work", "*", patch("mission", "New".into()))
        .unwrap();
    assert_eq!(new_notes.notes.entries[0].text, "New");
    assert_ne!(new_notes.notes.mission_id, old_notes.notes.mission_id);
}

#[test]
fn restore_port_refusals_preserve_revision_and_etag() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("work", "Work", "").unwrap();
    let edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "Work".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let formation = &edit.mission.formations[0];
    let repeated = Patch::RestorePort {
        formation_id: formation.id.clone(),
        direction: "input".into(),
        port: formation.inputs[0].clone(),
        index: Some(0),
        connections: vec![],
    };
    assert_eq!(
        store
            .patch("work", pre(&edit), repeated, "")
            .unwrap_err()
            .code
            .as_str(),
        "invalid_node_restore"
    );
    let invalid = Patch::RestorePort {
        formation_id: formation.id.clone(),
        direction: "in".into(),
        port: formation.inputs[0].clone(),
        index: Some(0),
        connections: vec![],
    };
    assert_eq!(
        store
            .patch("work", pre(&edit), invalid, "")
            .unwrap_err()
            .code
            .as_str(),
        "invalid_port_direction"
    );
    assert_eq!(store.mission_edit("work").unwrap().etag, edit.etag);
}

#[test]
fn formation_title_and_brief_commit_atomically() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let created = store.create_mission("work", "Work", "human:ui").unwrap();
    let created = store
        .patch(
            "work",
            pre(&created),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "Old".into(),
                x: 0,
                y: 0,
            },
            "human:ui",
        )
        .unwrap();
    let id = created.mission.formations[0].id.clone();
    let bad = parvi_mission_model::Brief {
        goal: "Goal".into(),
        files: vec!["relative".into()],
        ..Default::default()
    };
    assert!(
        store
            .patch(
                "work",
                pre(&created),
                Patch::EditFormation {
                    id: id.clone(),
                    title: "New".into(),
                    brief: Some(bad)
                },
                "human:ui"
            )
            .is_err()
    );
    assert_eq!(store.mission_edit("work").unwrap().etag, created.etag);
    let brief = parvi_mission_model::Brief {
        goal: "Goal".into(),
        ..Default::default()
    };
    let changed = store
        .patch(
            "work",
            pre(&created),
            Patch::EditFormation {
                id,
                title: "New".into(),
                brief: Some(brief.clone()),
            },
            "human:ui",
        )
        .unwrap();
    assert_eq!(changed.mission.rev, created.mission.rev + 1);
    assert_eq!(changed.mission.formations[0].title, "New");
    assert_eq!(changed.mission.formations[0].brief, Some(brief));
}

#[test]
fn swapping_staffing_is_one_revision_and_reversible_across_formations() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("work", "Work", "").unwrap();
    for title in ["First", "Second"] {
        edit = store
            .patch(
                "work",
                pre(&edit),
                Patch::CreateFormation {
                    kind: "solo".into(),
                    title: title.into(),
                    x: 0,
                    y: 0,
                },
                "",
            )
            .unwrap();
    }
    let a = &edit.mission.formations[0];
    let b = &edit.mission.formations[1];
    let (af, aslot, bf, bslot) = (
        a.id.clone(),
        a.slots[0].id.clone(),
        b.id.clone(),
        b.slots[0].id.clone(),
    );
    edit = store
        .patch(
            "work",
            pre(&edit),
            Patch::AssignSlot {
                formation_id: af.clone(),
                slot_id: aslot.clone(),
                agent_id: "builder".into(),
                harness: "claude-code".into(),
                model: "opus".into(),
                effort: "medium".into(),
            },
            "",
        )
        .unwrap();
    let swap = || Patch::SwapSlots {
        from_formation: af.clone(),
        from_slot: aslot.clone(),
        to_formation: bf.clone(),
        to_slot: bslot.clone(),
    };
    let before = edit.mission.rev;
    edit = store.patch("work", pre(&edit), swap(), "").unwrap();
    assert_eq!(edit.mission.rev, before + 1);
    assert_eq!(edit.mission.formations[0].slots[0].agent_id, "");
    assert_eq!(edit.mission.formations[1].slots[0].agent_id, "builder");
    edit = store.patch("work", pre(&edit), swap(), "").unwrap();
    assert_eq!(edit.mission.formations[0].slots[0].agent_id, "builder");
    assert_eq!(edit.mission.formations[1].slots[0].agent_id, "");
    let invalid = Patch::SwapSlots {
        from_formation: af,
        from_slot: aslot,
        to_formation: bf,
        to_slot: "missing".into(),
    };
    assert_eq!(
        store
            .patch("work", pre(&edit), invalid, "")
            .unwrap_err()
            .code
            .as_str(),
        "not_found"
    );
    assert_eq!(
        store.read_mission("work").unwrap().formations[0].slots[0].agent_id,
        "builder"
    );
}
