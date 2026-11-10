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

// Ported from formation_shape_test.go: both type restoration and node restore
// reject every invalid shape before touching either file.
#[test]
fn invalid_formation_shapes_are_refused_by_both_restore_paths() {
    use parvi_missions::{Formation, RestoredNode, Slot};
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("shape", "Shape", "").unwrap();
    let edit = store
        .patch(
            "shape",
            pre(&edit),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "Work".into(),
                x: 3,
                y: 4,
            },
            "",
        )
        .unwrap();
    let id = edit.mission.formations[0].id.clone();
    let mission_path = dir.0.join("missions/shape.mission.toml");
    let layout_path = dir.0.join("layout/shape.layout.toml");
    let mission_before = std::fs::read(&mission_path).unwrap();
    let layout_before = std::fs::read(&layout_path).unwrap();
    let slot = |name: &str, controller| Slot {
        id: name.into(),
        controller,
        ..Default::default()
    };
    let shapes = vec![
        ("solo", vec![]),
        ("solo", vec![slot("slot_a", false), slot("slot_b", false)]),
        ("peer", vec![]),
        ("peer", vec![slot("slot_a", false)]),
        ("peer", vec![slot("slot_a", true), slot("slot_b", false)]),
        ("orchestrated", vec![]),
        ("orchestrated", vec![slot("slot_a", true)]),
        (
            "orchestrated",
            vec![slot("slot_a", false), slot("slot_b", false)],
        ),
        (
            "orchestrated",
            vec![slot("slot_a", true), slot("slot_b", true)],
        ),
    ];
    for (kind, slots) in shapes {
        let err = store
            .patch(
                "shape",
                pre(&edit),
                Patch::SetFormationType {
                    id: id.clone(),
                    kind: kind.into(),
                    keep_slot_id: "".into(),
                    slots: Some(slots.clone()),
                },
                "",
            )
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_TYPE_CHANGE", "{kind}: {slots:?}");
        let err = store
            .patch(
                "shape",
                pre(&edit),
                Patch::RestoreNode {
                    node: RestoredNode::Formation(Formation {
                        id: "fmn_bad".into(),
                        kind: kind.into(),
                        slots: slots.clone(),
                        ..Default::default()
                    }),
                    connections: vec![],
                    index: None,
                    x: 0,
                    y: 0,
                },
                "",
            )
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_NODE_RESTORE", "{kind}: {slots:?}");
        assert_eq!(std::fs::read(&mission_path).unwrap(), mission_before);
        assert_eq!(std::fs::read(&layout_path).unwrap(), layout_before);
    }
}

// Ported from port_restore_test.go: each position in either direction survives
// remove/undo, including wires and original order.
#[test]
fn restore_each_input_and_output_port_at_its_original_index() {
    for (direction, index) in [("input", 0), ("input", 1), ("output", 0), ("output", 1)] {
        let dir = Scratch::new();
        let store = Store::new(&dir.0);
        let mut edit = store.create_mission("ports", "Ports", "").unwrap();
        for title in ["Source", "Target"] {
            edit = store
                .patch(
                    "ports",
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
        let source = edit.mission.formations[0].id.clone();
        let target = edit.mission.formations[1].id.clone();
        let edited_id = if direction == "input" {
            &target
        } else {
            &source
        };
        edit = store
            .patch(
                "ports",
                pre(&edit),
                Patch::AddPort {
                    formation_id: edited_id.clone(),
                    direction: direction.into(),
                    label: "Second".into(),
                    kind: "".into(),
                },
                "",
            )
            .unwrap();
        let before = edit
            .mission
            .formations
            .iter()
            .find(|f| &f.id == edited_id)
            .unwrap()
            .clone();
        let port = if direction == "input" {
            before.inputs[index].clone()
        } else {
            before.outputs[index].clone()
        };
        let from_port = if direction == "output" {
            &port
        } else {
            &edit.mission.formations[0].outputs[0]
        };
        let to_port = if direction == "input" {
            &port
        } else {
            &edit.mission.formations[1].inputs[0]
        };
        edit = store
            .patch(
                "ports",
                pre(&edit),
                Patch::WireConnection {
                    id: "edge_restore".into(),
                    from: format!("{source}:{}", from_port.id),
                    to: format!("{target}:{}", to_port.id),
                    join_if_occupied: false,
                },
                "",
            )
            .unwrap();
        let wire = edit.mission.connections[0].clone();
        let rev = edit.mission.rev;
        edit = store
            .patch(
                "ports",
                pre(&edit),
                Patch::RemovePort {
                    formation_id: edited_id.clone(),
                    port_id: port.id.clone(),
                },
                "",
            )
            .unwrap();
        assert!(edit.mission.connections.is_empty());
        edit = store
            .patch(
                "ports",
                pre(&edit),
                Patch::RestorePort {
                    formation_id: edited_id.clone(),
                    direction: direction.into(),
                    port: port.clone(),
                    index: Some(index),
                    connections: vec![wire.clone()],
                },
                "",
            )
            .unwrap();
        assert_eq!(edit.mission.rev, rev + 2);
        assert_eq!(
            edit.mission
                .formations
                .iter()
                .find(|f| &f.id == edited_id)
                .unwrap(),
            &before
        );
        assert_eq!(edit.mission.connections, vec![wire]);
        assert_eq!(
            store
                .patch(
                    "ports",
                    pre(&edit),
                    Patch::RestorePort {
                        formation_id: edited_id.clone(),
                        direction: direction.into(),
                        port,
                        index: Some(index),
                        connections: vec![]
                    },
                    ""
                )
                .unwrap_err()
                .code(),
            "INVALID_NODE_RESTORE"
        );
    }
}

// Ported from formation_type_test.go: exact slot snapshots are undo data.
#[test]
fn formation_type_restores_exact_slot_snapshot_and_rejects_bad_choices() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("type", "Type", "").unwrap();
    let mut edit = store
        .patch(
            "type",
            pre(&edit),
            Patch::CreateFormation {
                kind: "orchestrated".into(),
                title: "Desk".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let formation = edit.mission.formations[0].clone();
    let snapshot = formation.slots.clone();
    edit = store
        .patch(
            "type",
            pre(&edit),
            Patch::SetFormationType {
                id: formation.id.clone(),
                kind: "solo".into(),
                keep_slot_id: snapshot[1].id.clone(),
                slots: None,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "type",
            pre(&edit),
            Patch::SetFormationType {
                id: formation.id.clone(),
                kind: "orchestrated".into(),
                keep_slot_id: "".into(),
                slots: Some(snapshot.clone()),
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[0].slots, snapshot);
    let path = dir.0.join("missions/type.mission.toml");
    let before = std::fs::read(&path).unwrap();
    let duplicate = vec![snapshot[0].clone(), snapshot[0].clone()];
    for (id, kind, keep, slots, code) in [
        (
            formation.id.as_str(),
            "flow",
            "",
            None,
            "UNSUPPORTED_FORMATION_TYPE",
        ),
        (
            formation.id.as_str(),
            "swarm",
            "",
            None,
            "UNSUPPORTED_FORMATION_TYPE",
        ),
        (
            formation.id.as_str(),
            "peer",
            snapshot[0].id.as_str(),
            None,
            "INVALID_TYPE_CHANGE",
        ),
        (
            formation.id.as_str(),
            "solo",
            "slot_missing",
            None,
            "INVALID_TYPE_CHANGE",
        ),
        (
            formation.id.as_str(),
            "peer",
            "",
            Some(duplicate),
            "INVALID_TYPE_CHANGE",
        ),
        ("fmn_missing", "solo", "", None, "NOT_FOUND"),
    ] {
        assert_eq!(
            store
                .patch(
                    "type",
                    pre(&edit),
                    Patch::SetFormationType {
                        id: id.into(),
                        kind: kind.into(),
                        keep_slot_id: keep.into(),
                        slots
                    },
                    ""
                )
                .unwrap_err()
                .code(),
            code
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

// Ported from gate_update_test.go: dropping formation removes only judge wires.
#[test]
fn changing_gate_kinds_detaches_judge_without_touching_layout() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("judge", "Judge", "").unwrap();
    edit = store
        .patch(
            "judge",
            pre(&edit),
            Patch::CreateFormation {
                kind: "solo".into(),
                title: "Reviewer".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "judge",
            pre(&edit),
            Patch::CreateGate {
                title: "Gate".into(),
                kinds: vec!["code".into(), "formation".into()],
                criterion: "Review".into(),
                check: "output_contains".into(),
                check_version: "1".into(),
                check_value: "OK".into(),
                files: vec![],
                x: 2,
                y: 3,
            },
            "",
        )
        .unwrap();
    let judge = edit.mission.formations[0].id.clone();
    let gate = edit.mission.gates[0].id.clone();
    edit = store
        .patch(
            "judge",
            pre(&edit),
            Patch::SetGateJudge {
                gate_id: gate.clone(),
                chain: vec![judge],
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.connections.len(), 2);
    let layout = std::fs::read(dir.0.join("layout/judge.layout.toml")).unwrap();
    edit = store
        .patch(
            "judge",
            pre(&edit),
            Patch::UpdateGate(parvi_missions::GateUpdate {
                id: gate.clone(),
                kinds: Some(vec!["code".into(), "human".into()]),
                ..Default::default()
            }),
            "",
        )
        .unwrap();
    assert!(edit.mission.connections.is_empty());
    assert_eq!(edit.mission.gates[0].check_value, "OK");
    assert_eq!(
        std::fs::read(dir.0.join("layout/judge.layout.toml")).unwrap(),
        layout
    );
    edit = store
        .patch(
            "judge",
            pre(&edit),
            Patch::UpdateGate(parvi_missions::GateUpdate {
                id: gate,
                kinds: Some(vec!["human".into(), "formation".into()]),
                ..Default::default()
            }),
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.gates[0].check, "");
    assert_eq!(edit.mission.gates[0].check_value, "");
}

// Ported from formation_join_test.go: gates never grow another input and
// stale callers are rejected even when the current input is occupied.
#[test]
fn join_keeps_gate_single_feed_and_rejects_stale_writes() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("join-gate", "Join gate", "").unwrap();
    for title in ["One", "Two"] {
        edit = store
            .patch(
                "join-gate",
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
    edit = store
        .patch(
            "join-gate",
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
    let from = |i: usize| {
        format!(
            "{}:{}",
            edit.mission.formations[i].id, edit.mission.formations[i].outputs[0].id
        )
    };
    let first = from(0);
    let second = from(1);
    let to = format!("{}:in", edit.mission.gates[0].id);
    let stale = pre(&edit);
    let wire = |from: String| Patch::WireConnection {
        id: "".into(),
        from,
        to: to.clone(),
        join_if_occupied: true,
    };
    edit = store
        .patch("join-gate", stale.clone(), wire(first), "")
        .unwrap();
    assert_eq!(
        store
            .patch("join-gate", pre(&edit), wire(second.clone()), "")
            .unwrap_err()
            .code(),
        "INPUT_OCCUPIED"
    );
    assert_eq!(
        store
            .patch("join-gate", stale, wire(second), "")
            .unwrap_err()
            .code(),
        "CONFLICT"
    );
    assert_eq!(store.mission_edit("join-gate").unwrap().etag, edit.etag);
}

// Ported from formation_join_test.go: feedback may share an occupied input
// during rewire and undo without introducing a required extra port.
#[test]
fn feedback_rewire_and_join_undo_preserve_pushback_exception() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("feedback", "Feedback", "").unwrap();
    for title in ["Frame", "Research", "Ship"] {
        edit = store
            .patch(
                "feedback",
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
    edit = store
        .patch(
            "feedback",
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
    let endpoint = |i: usize, output: bool| {
        let f = &edit.mission.formations[i];
        format!(
            "{}:{}",
            f.id,
            if output {
                &f.outputs[0].id
            } else {
                &f.inputs[0].id
            }
        )
    };
    let (frame_out, research_in, ship_in, ship_out) = (
        endpoint(0, true),
        endpoint(1, false),
        endpoint(2, false),
        endpoint(2, true),
    );
    let fail = format!("{}:fail", edit.mission.gates[0].id);
    let wire = |from: &str, to: &str| Patch::WireConnection {
        id: "".into(),
        from: from.into(),
        to: to.into(),
        join_if_occupied: false,
    };
    edit = store
        .patch("feedback", pre(&edit), wire(&frame_out, &ship_in), "")
        .unwrap();
    edit = store
        .patch("feedback", pre(&edit), wire(&fail, &research_in), "")
        .unwrap();
    edit = store
        .patch(
            "feedback",
            pre(&edit),
            Patch::RewireConnection {
                from: fail.clone(),
                previous_to: research_in.clone(),
                to: ship_in.clone(),
                join_if_occupied: true,
                remove_previous_input: false,
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[2].inputs.len(), 1);
    assert!(
        edit.mission
            .connections
            .iter()
            .any(|c| c.from == fail && c.to == ship_in)
    );
    edit = store
        .patch("feedback", pre(&edit), wire(&ship_out, &research_in), "")
        .unwrap();
    edit = store
        .patch(
            "feedback",
            pre(&edit),
            Patch::RewireConnection {
                from: frame_out.clone(),
                previous_to: ship_in.clone(),
                to: research_in,
                join_if_occupied: true,
                remove_previous_input: false,
            },
            "",
        )
        .unwrap();
    let joined = edit
        .mission
        .connections
        .iter()
        .find(|c| c.from == frame_out)
        .unwrap()
        .to
        .clone();
    assert_eq!(edit.mission.formations[1].inputs.len(), 2);
    edit = store
        .patch(
            "feedback",
            pre(&edit),
            Patch::RewireConnection {
                from: frame_out.clone(),
                previous_to: joined,
                to: ship_in.clone(),
                join_if_occupied: false,
                remove_previous_input: true,
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[1].inputs.len(), 1);
    assert!(
        edit.mission
            .connections
            .iter()
            .any(|c| c.from == frame_out && c.to == ship_in)
    );
    assert!(
        edit.mission
            .connections
            .iter()
            .any(|c| c.from == fail && c.to == ship_in)
    );
}

// Ported from node_restore_test.go: failed undo does not publish either file.
#[test]
fn restore_refuses_missing_foreign_and_occupied_connections_and_bad_nodes() {
    use parvi_missions::{Connection, Formation, Gate, RestoredNode};
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("restore", "Restore", "").unwrap();
    for title in ["Build", "Other", "Target"] {
        edit = store
            .patch(
                "restore",
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
    let build = edit.mission.formations[0].clone();
    let other = edit.mission.formations[1].clone();
    let target = edit.mission.formations[2].clone();
    let output = |f: &Formation| format!("{}:{}", f.id, f.outputs[0].id);
    let input = |f: &Formation| format!("{}:{}", f.id, f.inputs[0].id);
    edit = store
        .patch(
            "restore",
            pre(&edit),
            Patch::WireConnection {
                id: "edge_occupied".into(),
                from: output(&other),
                to: input(&target),
                join_if_occupied: false,
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "restore",
            pre(&edit),
            Patch::DeleteFormation {
                id: build.id.clone(),
            },
            "",
        )
        .unwrap();
    let mission_path = dir.0.join("missions/restore.mission.toml");
    let layout_path = dir.0.join("layout/restore.layout.toml");
    let mission_before = std::fs::read(&mission_path).unwrap();
    let layout_before = std::fs::read(&layout_path).unwrap();
    let connection = |from: String, to: String| Connection {
        id: "edge_x".into(),
        from,
        to,
    };
    let cases = vec![
        (
            RestoredNode::Formation(other.clone()),
            vec![],
            "INVALID_NODE_RESTORE",
        ),
        (
            RestoredNode::Formation(build.clone()),
            vec![connection(output(&build), "gate_missing:in".into())],
            "INVALID_NODE_RESTORE",
        ),
        (
            RestoredNode::Formation(build.clone()),
            vec![connection(output(&other), input(&target))],
            "INVALID_NODE_RESTORE",
        ),
        (
            RestoredNode::Formation(build.clone()),
            vec![connection(output(&build), input(&target))],
            "INVALID_NODE_RESTORE",
        ),
        (
            RestoredNode::Formation(Formation {
                id: "fmn_old".into(),
                kind: "flow".into(),
                ..Default::default()
            }),
            vec![],
            "UNSUPPORTED_FORMATION_TYPE",
        ),
        (
            RestoredNode::Gate(Gate {
                id: "gate_new".into(),
                ..Default::default()
            }),
            vec![],
            "INVALID_GATE_KIND",
        ),
        (
            RestoredNode::Gate(Gate {
                id: "gate_new".into(),
                kinds: vec!["human".into()],
                check_value: "PASS".into(),
                ..Default::default()
            }),
            vec![],
            "INVALID_CODE_GATE_PROFILE",
        ),
        (
            RestoredNode::Formation(Formation {
                id: "fmn_copy".into(),
                kind: "solo".into(),
                slots: other.slots.clone(),
                ..Default::default()
            }),
            vec![],
            "INVALID_NODE_RESTORE",
        ),
    ];
    for (node, connections, code) in cases {
        let error = store
            .patch(
                "restore",
                pre(&edit),
                Patch::RestoreNode {
                    node,
                    connections,
                    index: None,
                    x: 0,
                    y: 0,
                },
                "",
            )
            .unwrap_err();
        assert_eq!(error.code(), code);
        assert_eq!(std::fs::read(&mission_path).unwrap(), mission_before);
        assert_eq!(std::fs::read(&layout_path).unwrap(), layout_before);
    }
}

// Ported from store_test.go: judge chains have a single send and return,
// can extend the return, and cannot occupy an already-fed input.
#[test]
fn judge_chain_extends_its_return_and_refuses_occupied_inputs() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("chain", "Chain", "").unwrap();
    for title in ["Judge 1", "Judge 2", "Judge 3", "Other"] {
        edit = store
            .patch(
                "chain",
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
    edit = store
        .patch(
            "chain",
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
    let formations = edit.mission.formations.clone();
    let gate = edit.mission.gates[0].id.clone();
    let endpoint = |i: usize, output: bool| {
        let f = &formations[i];
        format!(
            "{}:{}",
            f.id,
            if output {
                &f.outputs[0].id
            } else {
                &f.inputs[0].id
            }
        )
    };
    edit = store
        .patch(
            "chain",
            pre(&edit),
            Patch::WireConnection {
                id: "".into(),
                from: endpoint(3, true),
                to: endpoint(0, false),
                join_if_occupied: false,
            },
            "",
        )
        .unwrap();
    let chain = |count: usize| Patch::SetGateJudge {
        gate_id: gate.clone(),
        chain: formations
            .iter()
            .take(count)
            .map(|f| f.id.clone())
            .collect(),
    };
    assert_eq!(
        store
            .patch("chain", pre(&edit), chain(1), "")
            .unwrap_err()
            .code(),
        "INPUT_OCCUPIED"
    );
    assert_eq!(store.mission_edit("chain").unwrap().etag, edit.etag);
    edit = store
        .patch(
            "chain",
            pre(&edit),
            Patch::UnwireConnection {
                from: endpoint(3, true),
                to: endpoint(0, false),
            },
            "",
        )
        .unwrap();
    edit = store.patch("chain", pre(&edit), chain(2), "").unwrap();
    assert_eq!(edit.mission.connections.len(), 3);
    let gate_endpoint = format!("{gate}:judge");
    assert_eq!(
        edit.mission
            .connections
            .iter()
            .filter(|c| c.from == gate_endpoint)
            .count(),
        1
    );
    assert_eq!(
        edit.mission
            .connections
            .iter()
            .filter(|c| c.to == gate_endpoint)
            .count(),
        1
    );
    edit = store.patch("chain", pre(&edit), chain(3), "").unwrap();
    assert!(
        edit.mission
            .connections
            .iter()
            .any(|c| c.from == gate_endpoint && c.to == endpoint(0, false))
    );
    assert!(
        edit.mission
            .connections
            .iter()
            .any(|c| c.from == endpoint(2, true) && c.to == gate_endpoint)
    );
}

// Port of TestS3RewireRejectsOccupiedTargetWithoutDroppingOriginal.
#[test]
fn refused_rewire_retains_original_and_occupied_connections() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("rewire", "Rewire", "").unwrap();
    for title in ["Frame", "Research", "Ship"] {
        edit = store
            .patch(
                "rewire",
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
    let endpoint = |i: usize, output: bool| {
        let f = &edit.mission.formations[i];
        format!(
            "{}:{}",
            f.id,
            if output {
                &f.outputs[0].id
            } else {
                &f.inputs[0].id
            }
        )
    };
    let (frame_out, research_in, research_out, ship_in) = (
        endpoint(0, true),
        endpoint(1, false),
        endpoint(1, true),
        endpoint(2, false),
    );
    for (from, to) in [(&frame_out, &research_in), (&research_out, &ship_in)] {
        edit = store
            .patch(
                "rewire",
                pre(&edit),
                Patch::WireConnection {
                    id: "".into(),
                    from: from.clone(),
                    to: to.clone(),
                    join_if_occupied: false,
                },
                "",
            )
            .unwrap();
    }
    let original = edit.mission.connections.clone();
    let error = store
        .patch(
            "rewire",
            pre(&edit),
            Patch::RewireConnection {
                from: frame_out,
                previous_to: research_in,
                to: ship_in,
                join_if_occupied: false,
                remove_previous_input: false,
            },
            "",
        )
        .unwrap_err();
    assert_eq!(error.code(), "INPUT_OCCUPIED");
    let after = store.mission_edit("rewire").unwrap();
    assert_eq!(after.etag, edit.etag);
    assert_eq!(after.mission.connections, original);
}

// Port of TestS3DynamicPortsKeepStableIDsAndPruneEdges.
#[test]
fn removing_a_dynamic_port_prunes_only_its_connections() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let mut edit = store.create_mission("ports", "Ports", "").unwrap();
    for title in ["A", "B", "Target"] {
        edit = store
            .patch(
                "ports",
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
    let target = edit.mission.formations[2].id.clone();
    edit = store
        .patch(
            "ports",
            pre(&edit),
            Patch::AddPort {
                formation_id: target.clone(),
                direction: "input".into(),
                label: "Second".into(),
                kind: "".into(),
            },
            "",
        )
        .unwrap();
    let ports = edit.mission.formations[2].inputs.clone();
    assert!(ports[1].id.starts_with("port_"));
    assert_ne!(ports[0].id, ports[1].id);
    let sources: Vec<_> = edit.mission.formations[..2]
        .iter()
        .map(|f| format!("{}:{}", f.id, f.outputs[0].id))
        .collect();
    for i in 0..2 {
        edit = store
            .patch(
                "ports",
                pre(&edit),
                Patch::WireConnection {
                    id: format!("edge_{i}"),
                    from: sources[i].clone(),
                    to: format!("{target}:{}", ports[i].id),
                    join_if_occupied: false,
                },
                "",
            )
            .unwrap();
    }
    edit = store
        .patch(
            "ports",
            pre(&edit),
            Patch::RemovePort {
                formation_id: target,
                port_id: ports[1].id.clone(),
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[2].inputs, vec![ports[0].clone()]);
    assert_eq!(edit.mission.connections.len(), 1);
    assert_eq!(edit.mission.connections[0].id, "edge_0");
}

// Port of TestPeerSnapshotCanRemoveOnlyEmptySeats.
#[test]
fn peer_snapshot_cannot_silently_remove_a_staffed_seat() {
    let dir = Scratch::new();
    let store = Store::new(&dir.0);
    let edit = store.create_mission("peer", "Peer", "").unwrap();
    let mut edit = store
        .patch(
            "peer",
            pre(&edit),
            Patch::CreateFormation {
                kind: "peer".into(),
                title: "Team".into(),
                x: 0,
                y: 0,
            },
            "",
        )
        .unwrap();
    let team = edit.mission.formations[0].clone();
    edit = store
        .patch(
            "peer",
            pre(&edit),
            Patch::AssignSlot {
                formation_id: team.id.clone(),
                slot_id: team.slots[0].id.clone(),
                agent_id: "builder".into(),
                harness: "claude-code".into(),
                model: "".into(),
                effort: "medium".into(),
            },
            "",
        )
        .unwrap();
    // A peer must keep at least two seats; add one so a snapshot can remove one.
    let mut three = edit.mission.formations[0].slots.clone();
    three.push(parvi_missions::Slot {
        id: "slot_third".into(),
        label: "Third".into(),
        ..Default::default()
    });
    edit = store
        .patch(
            "peer",
            pre(&edit),
            Patch::SetFormationType {
                id: team.id.clone(),
                kind: "peer".into(),
                keep_slot_id: "".into(),
                slots: Some(three.clone()),
            },
            "",
        )
        .unwrap();
    let remaining = three[1..].to_vec();
    let refused = store
        .patch(
            "peer",
            pre(&edit),
            Patch::SetFormationType {
                id: team.id.clone(),
                kind: "peer".into(),
                keep_slot_id: "".into(),
                slots: Some(remaining.clone()),
            },
            "",
        )
        .unwrap_err();
    assert_eq!(refused.code(), "SLOT_CHOICE_REQUIRED");
    assert_eq!(store.mission_edit("peer").unwrap().etag, edit.etag);
    edit = store
        .patch(
            "peer",
            pre(&edit),
            Patch::AssignSlot {
                formation_id: team.id.clone(),
                slot_id: team.slots[0].id.clone(),
                agent_id: "".into(),
                harness: "".into(),
                model: "".into(),
                effort: "".into(),
            },
            "",
        )
        .unwrap();
    edit = store
        .patch(
            "peer",
            pre(&edit),
            Patch::SetFormationType {
                id: team.id,
                kind: "peer".into(),
                keep_slot_id: "".into(),
                slots: Some(remaining.clone()),
            },
            "",
        )
        .unwrap();
    assert_eq!(edit.mission.formations[0].slots, remaining);
}
