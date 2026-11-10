use parvi_missions::{Patch, Precondition, Store};
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
fn pre(edit: &parvi_missions::Edit) -> Precondition {
    Precondition {
        rev: edit.mission.rev,
        etag: edit.etag.clone(),
    }
}

// Ported from Archon's store_test.go: creation must not clobber a definition.
#[test]
fn create_mission_defaults_title_and_refuses_duplicate_without_clobber() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("poems", "", "agent:test").unwrap();
    assert_eq!(edit.mission.title, "poems");
    assert_eq!(edit.mission.rev, 1);
    assert!(edit.mission.id.starts_with("msn_"));
    let path = dir.0.join("missions/poems.mission.toml");
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        store
            .create_mission("poems", "Different", "")
            .unwrap_err()
            .code(),
        "ALREADY_EXISTS"
    );
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(store.create_mission("bad/path", "", "").is_err());
}

// Ported from store_test.go: wildcard can recreate only the missing layout.
#[test]
fn missing_layout_can_be_recreated_without_dirtying_the_mission() {
    use parvi_missions::{LayoutNode, LayoutPatch};
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("layout", "Layout", "").unwrap();
    let path = dir.0.join("missions/layout.mission.toml");
    let before = std::fs::read(&path).unwrap();
    let nodes = vec![LayoutNode {
        id: "fmn_example".into(),
        x: 7,
        y: -9,
    }];
    assert_eq!(
        store
            .patch_layout("layout", "old", LayoutPatch::Nodes(nodes.clone()))
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
    assert_eq!(
        store
            .patch_layout("layout", "*", LayoutPatch::Edges(vec![]))
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
    let saved = store
        .patch_layout("layout", "*", LayoutPatch::Nodes(nodes.clone()))
        .unwrap();
    assert_eq!(saved.layout.nodes, nodes);
    assert_eq!(saved.layout.mission_rev, edit.mission.rev);
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(
        store
            .patch_layout("layout", "*", LayoutPatch::Nodes(nodes))
            .unwrap_err()
            .code(),
        "CONFLICT"
    );
}

// Ported from toml_value_fidelity_test.go: valid TOML strings must be decoded,
// and changing one field must retain unrelated multiline values and comments.
#[test]
fn editing_decoded_multiline_toml_keeps_unrelated_values() {
    let dir = Scratch::new();
    std::fs::create_dir_all(dir.0.join("missions")).unwrap();
    let path = dir.0.join("missions/quoted.mission.toml");
    let raw = r###"schema = 1
id = 'msn_quoted'
slug = 'quoted'
"ti\u0074le" = """
Old title
continued
""" # keep title comment
rev = 1

[[inputCard]]
id = 'inp_main'
title = 'Frame #1'
goal = """
Line one
Line \u0023 two
""" # preserve goal
"###;
    std::fs::write(&path, raw).unwrap();
    let store = Store::new(&dir.0);
    let edit = store.mission_edit("quoted").unwrap();
    assert_eq!(edit.mission.input_cards[0].goal, "Line one\nLine # two\n");
    let updated = store
        .patch("quoted", pre(&edit), Patch::Title("New title".into()), "")
        .unwrap();
    assert_eq!(updated.mission.title, "New title");
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(!after.contains("Old title"));
    assert!(after.contains("# keep title comment"));
    assert!(after.contains("Line \\u0023 two"));
    assert!(after.contains("# preserve goal"));
}

// Archon's unknown-field preservation cases are represented as strict-source
// refusal: Parvi's established schema deliberately rejects unknown fields.
#[test]
fn invalid_definition_source_is_never_changed_by_an_edit() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("invalid", "Invalid", "").unwrap();
    let path = dir.0.join("missions/invalid.mission.toml");
    let raw = format!(
        "{}futureRoot = '''\nunknown # text\n'''\n",
        std::fs::read_to_string(&path).unwrap()
    );
    std::fs::write(&path, &raw).unwrap();
    assert!(
        store
            .patch("invalid", pre(&edit), Patch::Title("Changed".into()), "")
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), raw);
}

#[cfg(unix)]
#[test]
fn writes_follow_a_symlinked_definition_directory() {
    use std::os::unix::fs::symlink;
    let dir = Scratch::new();
    let target = dir.0.join("actual-missions");
    std::fs::create_dir_all(&target).unwrap();
    symlink("actual-missions", dir.0.join("missions")).unwrap();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("linked", "Linked", "").unwrap();
    let edit = store
        .patch("linked", pre(&edit), Patch::Title("Updated".into()), "")
        .unwrap();
    assert_eq!(edit.mission.title, "Updated");
    assert!(
        std::fs::symlink_metadata(dir.0.join("missions"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        std::fs::read_to_string(target.join("linked.mission.toml"))
            .unwrap()
            .contains("Updated")
    );
}

// Ported from notes_test.go: competing editors cannot overwrite a note thread.
#[test]
fn notes_append_in_order_and_stale_writers_do_not_clobber() {
    use parvi_missions::NotePatch;
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mission = store.create_mission("notes", "Notes", "").unwrap();
    let mission_path = dir.0.join("missions/notes.mission.toml");
    let mission_before = std::fs::read(&mission_path).unwrap();
    let append = |author: &str, text: &str| NotePatch::Append {
        target: "mission".into(),
        author: author.into(),
        text: text.into(),
    };
    let one = store
        .patch_notes("notes", "*", append("human:one", "First"))
        .unwrap();
    let two = store
        .patch_notes("notes", &one.etag, append("agent:two", "Second"))
        .unwrap();
    let path = dir.0.join("notes/notes.notes.toml");
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        store
            .patch_notes("notes", &one.etag, append("human:one", "Stale"))
            .unwrap_err()
            .code(),
        "CONFLICT"
    );
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(
        two.notes
            .entries
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second"]
    );
    assert_eq!(std::fs::read(mission_path).unwrap(), mission_before);
    assert_eq!(store.mission_edit("notes").unwrap().etag, mission.etag);
}

// Ported from toml_value_fidelity_test.go: paired creates and deletes must
// reject a malformed layout before either document is published.
#[test]
fn invalid_layout_blocks_every_paired_node_create_and_delete() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("paired", "Paired", "").unwrap();
    edit = store
        .patch(
            "paired",
            pre(&edit),
            Patch::CreateInputCard {
                title: "Input".into(),
                goal: "".into(),
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
            "paired",
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
            "paired",
            pre(&edit),
            Patch::CreateGate {
                title: "Review".into(),
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
    let path = dir.0.join("missions/paired.mission.toml");
    let layout_path = dir.0.join("layout/paired.layout.toml");
    let mission_before = std::fs::read(&path).unwrap();
    let invalid = format!(
        "schema = 1\nmissionId = {:?}\n\"mission\\u0049d\" = 'duplicate'\nmissionRev = 1\n",
        edit.mission.id
    );
    std::fs::write(&layout_path, &invalid).unwrap();
    let cases = vec![
        Patch::CreateInputCard {
            title: "New".into(),
            goal: "".into(),
            files: vec![],
            human_channel: "".into(),
            x: 0,
            y: 0,
        },
        Patch::CreateFormation {
            kind: "solo".into(),
            title: "New".into(),
            x: 0,
            y: 0,
        },
        Patch::CreateGate {
            title: "New".into(),
            kinds: vec!["human".into()],
            criterion: "".into(),
            check: "".into(),
            check_version: "".into(),
            check_value: "".into(),
            files: vec![],
            x: 0,
            y: 0,
        },
        Patch::DeleteInputCard {
            id: edit.mission.input_cards[0].id.clone(),
        },
        Patch::DeleteFormation {
            id: edit.mission.formations[0].id.clone(),
        },
        Patch::DeleteGate {
            id: edit.mission.gates[0].id.clone(),
        },
    ];
    for patch in cases {
        assert!(store.patch("paired", pre(&edit), patch, "").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), mission_before);
        assert_eq!(std::fs::read_to_string(&layout_path).unwrap(), invalid);
    }
}

// Port of TestBoardNoteThreadsAppendAndLimitEditsToTheAuthor.
#[test]
fn note_threads_allow_only_the_author_to_edit_or_delete() {
    use parvi_missions::NotePatch;
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("thread", "Thread", "").unwrap();
    let edit = store
        .patch(
            "thread",
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
    let target = edit.mission.formations[0].id.clone();
    let first = store
        .patch_notes(
            "thread",
            "*",
            NotePatch::Append {
                target: target.clone(),
                author: "human:ui".into(),
                text: "Map first".into(),
            },
        )
        .unwrap();
    let human = first.notes.entries[0].id.clone();
    let second = store
        .patch_notes(
            "thread",
            &first.etag,
            NotePatch::Append {
                target: target.clone(),
                author: "agent:worker".into(),
                text: "Agreed".into(),
            },
        )
        .unwrap();
    let agent = second.notes.entries[1].id.clone();
    assert_ne!(human, agent);
    let unauthorized = NotePatch::Edit {
        target: target.clone(),
        author: "agent:worker".into(),
        id: human.clone(),
        text: "Overwrite".into(),
    };
    assert_eq!(
        store
            .patch_notes("thread", &second.etag, unauthorized)
            .unwrap_err()
            .code(),
        "NOTE_AUTHOR_MISMATCH"
    );
    let updated = store
        .patch_notes(
            "thread",
            &second.etag,
            NotePatch::Edit {
                target: target.clone(),
                author: "agent:worker".into(),
                id: agent.clone(),
                text: "Agreed; add a planner".into(),
            },
        )
        .unwrap();
    assert_eq!(updated.notes.entries[0].text, "Map first");
    assert_eq!(updated.notes.entries[1].id, agent);
    assert_eq!(updated.notes.entries[1].text, "Agreed; add a planner");
    assert!(updated.notes.entries[1].edited_at.is_some());
    let deleted = store
        .patch_notes(
            "thread",
            &updated.etag,
            NotePatch::Delete {
                target: target.clone(),
                author: "agent:worker".into(),
                id: agent,
            },
        )
        .unwrap();
    assert_eq!(deleted.notes.entries.len(), 1);
    let cleared = store
        .patch_notes(
            "thread",
            &deleted.etag,
            NotePatch::Delete {
                target,
                author: "human:ui".into(),
                id: human,
            },
        )
        .unwrap();
    assert!(cleared.notes.entries.is_empty());
}

#[test]
fn partial_layout_patches_keep_other_nodes_and_edges() {
    use parvi_missions::{LayoutEdge, LayoutNode, LayoutPatch};
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    store.create_mission("canvas", "Canvas", "").unwrap();
    let node = |id: &str, x, y| LayoutNode {
        id: id.into(),
        x,
        y,
    };
    let edge = |id: &str, lane: &str| LayoutEdge {
        id: id.into(),
        lane: lane.into(),
    };
    let layout = store
        .patch_layout(
            "canvas",
            "*",
            LayoutPatch::Nodes(vec![node("a", 1, 2), node("b", 3, 4)]),
        )
        .unwrap();
    let layout = store
        .patch_layout(
            "canvas",
            &layout.etag,
            LayoutPatch::Edges(vec![edge("e1", "top"), edge("e2", "bottom")]),
        )
        .unwrap();
    let layout = store
        .patch_layout(
            "canvas",
            &layout.etag,
            LayoutPatch::Nodes(vec![node("a", 20, 30), node("c", 5, 6)]),
        )
        .unwrap();
    assert_eq!(
        layout.layout.nodes,
        vec![node("a", 20, 30), node("b", 3, 4), node("c", 5, 6)]
    );
    assert_eq!(
        layout.layout.edges,
        vec![edge("e1", "top"), edge("e2", "bottom")]
    );
    let layout = store
        .patch_layout(
            "canvas",
            &layout.etag,
            LayoutPatch::Edges(vec![edge("e1", "middle"), edge("e3", "new")]),
        )
        .unwrap();
    assert_eq!(
        layout.layout.edges,
        vec![
            edge("e1", "middle"),
            edge("e2", "bottom"),
            edge("e3", "new")
        ]
    );
    assert_eq!(layout.layout.nodes[1], node("b", 3, 4));
}

#[test]
fn empty_and_populated_inline_node_arrays_are_editable() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    store.create_mission("inline", "Inline", "").unwrap();
    let path = dir.0.join("missions/inline.mission.toml");
    let raw = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{raw}formation = []\ngate = [{{ id = 'gate_existing', title = 'Review', kinds = ['human'] }}]\nconnection = []\n")).unwrap();
    let edit = store.mission_edit("inline").unwrap();
    let edit = store
        .patch(
            "inline",
            pre(&edit),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "Worker".into(),
                x: 4,
                y: 5,
            },
            "",
        )
        .unwrap();
    let formation = edit.mission.formations[0].id.clone();
    let edit = store
        .patch(
            "inline",
            pre(&edit),
            Patch::UpdateFormation {
                id: formation.clone(),
                title: Some("Renamed".into()),
                device: None,
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[0].title, "Renamed");
    let edit = store
        .patch(
            "inline",
            pre(&edit),
            Patch::UpdateGate(parvi_missions::GateUpdate {
                id: "gate_existing".into(),
                criterion: Some("Approved".into()),
                ..Default::default()
            }),
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.gates[0].criterion, "Approved");
    let edit = store
        .patch(
            "inline",
            pre(&edit),
            Patch::DeleteFormation { id: formation },
            "",
        )
        .unwrap();
    assert!(edit.mission.formations.is_empty());
    let edit = store
        .patch(
            "inline",
            pre(&edit),
            Patch::DeleteGate {
                id: "gate_existing".into(),
            },
            "",
        )
        .unwrap();
    assert!(edit.mission.gates.is_empty());
}

#[test]
fn populated_inline_formations_and_connections_can_be_updated_and_deleted() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    store.create_mission("inline", "Inline", "").unwrap();
    let path = dir.0.join("missions/inline.mission.toml");
    let raw = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{raw}formation = [{{ id = 'fmn_a', type = 'solo', title = 'A', slot = [{{ id = 'slot_a', label = 'Worker' }}], output = [{{ id = 'port_out', label = 'Out' }}] }}, {{ id = 'fmn_b', type = 'solo', title = 'B', slot = [{{ id = 'slot_b', label = 'Worker' }}], input = [{{ id = 'port_in', label = 'In' }}] }}]\nconnection = [{{ id = 'edge_a', from = 'fmn_a:port_out', to = 'fmn_b:port_in' }}]\n")).unwrap();
    let edit = store.mission_edit("inline").unwrap();
    let edit = store
        .patch(
            "inline",
            pre(&edit),
            Patch::UpdateFormation {
                id: "fmn_a".into(),
                title: Some("Updated".into()),
                device: None,
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[0].title, "Updated");
    // A step is placed on an environment and back on the coordinator (#124).
    let place = |edit: &parvi_missions::Edit, device: &str| {
        store
            .patch(
                "inline",
                pre(edit),
                Patch::UpdateFormation {
                    id: "fmn_a".into(),
                    title: None,
                    device: Some(device.into()),
                },
                "",
            )
            .unwrap()
    };
    let edit = place(&edit, " landmass/windows ");
    assert_eq!(edit.mission.formations[0].device, "landmass/windows");
    let edit = place(&edit, "");
    assert_eq!(edit.mission.formations[0].device, "");
    assert!(!std::fs::read_to_string(&path).unwrap().contains("device"));
    let edit = store
        .patch(
            "inline",
            pre(&edit),
            Patch::DeleteFormation { id: "fmn_a".into() },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations.len(), 1);
    assert!(edit.mission.connections.is_empty());
}

#[test]
fn inline_note_entries_can_be_edited_and_deleted() {
    use parvi_missions::NotePatch;
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mission = store.create_mission("inline-notes", "Notes", "").unwrap();
    let notes_dir = dir.0.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    std::fs::write(notes_dir.join("inline-notes.notes.toml"), format!("schema = 2\nmissionId = {:?}\nrev = 1\nupdatedAt = '2026-01-01T00:00:00Z'\nentry = [{{ id = 'nte_a', target = 'mission', author = 'human:ui', createdAt = '2026-01-01T00:00:00Z', text = 'Before' }}]\n", mission.mission.id)).unwrap();
    let notes = store.notes_edit("inline-notes").unwrap();
    let notes = store
        .patch_notes(
            "inline-notes",
            &notes.etag,
            NotePatch::Edit {
                target: "mission".into(),
                author: "human:ui".into(),
                id: "nte_a".into(),
                text: "After".into(),
            },
        )
        .unwrap();
    assert_eq!(notes.notes.entries[0].text, "After");
    let notes = store
        .patch_notes(
            "inline-notes",
            &notes.etag,
            NotePatch::Delete {
                target: "mission".into(),
                author: "human:ui".into(),
                id: "nte_a".into(),
            },
        )
        .unwrap();
    assert!(notes.notes.entries.is_empty());
}

#[test]
fn inline_layout_nodes_and_edges_accept_partial_updates() {
    use parvi_missions::{LayoutEdge, LayoutNode, LayoutPatch};
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mission = store.create_mission("inline-layout", "Layout", "").unwrap();
    let layout_dir = dir.0.join("layout");
    std::fs::create_dir_all(&layout_dir).unwrap();
    std::fs::write(layout_dir.join("inline-layout.layout.toml"), format!("schema = 1\nmissionId = {:?}\nmissionRev = 1\nnode = [{{ id = 'a', x = 1, y = 2 }}, {{ id = 'b', x = 3, y = 4 }}]\nedge = [{{ id = 'e1', lane = 'top' }}, {{ id = 'e2', lane = 'bottom' }}]\n", mission.mission.id)).unwrap();
    let layout = store.layout_edit("inline-layout").unwrap();
    let layout = store
        .patch_layout(
            "inline-layout",
            &layout.etag,
            LayoutPatch::Nodes(vec![LayoutNode {
                id: "a".into(),
                x: 20,
                y: 30,
            }]),
        )
        .unwrap();
    assert_eq!(layout.layout.nodes[1].x, 3);
    let layout = store
        .patch_layout(
            "inline-layout",
            &layout.etag,
            LayoutPatch::Edges(vec![LayoutEdge {
                id: "e1".into(),
                lane: "middle".into(),
            }]),
        )
        .unwrap();
    assert_eq!(layout.layout.edges[1].lane, "bottom");
    assert_eq!(layout.layout.nodes[0].x, 20);
}
