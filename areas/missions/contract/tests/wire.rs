use parvi_missions_contract::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn round<T: Serialize + DeserializeOwned>(v: Value) {
    let parsed: T = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), v);
}

#[test]
fn requests_and_responses_round_trip() {
    round::<CreateMission>(json!({"title":"Demo","slug":"demo","updatedBy":"human:test"}));
    round::<DeleteMission>(json!({"expectedRev":2}));
    round::<EditNotes>(
        json!({"target":"mission","author":"human:test","text":"hi","entryId":"n","action":"edit"}),
    );
    round::<EditLayout>(json!({"nodes":[{"id":"a","x":1,"y":2}]}));
    round::<Abort>(json!({"requestedBy":"human:test","reason":"stop"}));
    round::<Land>(json!({"force":true}));
    round::<RunId>(json!({"runId":"r"}));
    round::<Landed>(json!({"landed":true,"notes":["done"]}));
    round::<DeletionResponse>(
        json!({"deletion":{"id":"m","slug":"demo","title":"Demo","archiveId":"a"}}),
    );
    round::<Vec<AgentSeat>>(
        json!([{"dispatchId":"d","slotId":"s","node":"local","session":"s","pane":2,"sessionName":"s","outcome":"","available":true,"waiting":{"state":"waiting","detail":"detail","since":"now","seq":1}}]),
    );
    round::<ValidationResponse>(
        json!({"missionRev":1,"missionEtag":"e","errors":[],"warnings":[]}),
    );
    round::<NotesResponse>(
        json!({"notes":{"schema":2,"missionId":"m","rev":1,"updatedAt":"2026-01-01T00:00:00Z","mission":[{"id":"n","author":"human:test","createdAt":"2026-01-01T00:00:00Z","text":"hello"}],"elements":[],"etag":"e"}}),
    );
}

#[test]
fn mission_layout_and_edit_documents_use_http_names() {
    let mission = Mission {
        schema: 1,
        id: "m".into(),
        formations: vec![Formation {
            id: "f".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let response = MissionResponse {
        mission: document::MissionDocument {
            mission,
            etag: Some("e".into()),
        },
    };
    let value = serde_json::to_value(&response).unwrap();
    assert!(value["mission"].get("inputCards").is_some());
    assert!(value["mission"]["formations"][0].get("slots").is_some());
    assert_eq!(value["mission"]["tools"], json!([]));
    round::<MissionResponse>(value.clone());
    let layout = document::LayoutDocument {
        layout: Layout {
            schema: 1,
            ..Default::default()
        },
        etag: "l".into(),
    };
    let edit = EditResponse {
        mission: response.mission,
        layout,
        changed: [("gateId".into(), json!("g"))].into(),
    };
    let value = serde_json::to_value(edit).unwrap();
    assert!(value["layout"].get("nodes").is_some());
    round::<EditResponse>(value);
}

#[test]
fn patch_and_frames_round_trip() {
    let patch: patch::UniquePatch = serde_json::from_value(
        json!({"expectedRev":2,"createGate":{"title":"Review","kinds":["human"]}}),
    )
    .unwrap();
    assert!(matches!(
        patch::decode(&patch.0).unwrap(),
        Patch::CreateGate { .. }
    ));
    round::<patch::UniquePatch>(serde_json::to_value(patch).unwrap());
    assert!(
        serde_json::from_str::<patch::UniquePatch>(r#"{"title":"one","title":"two"}"#).is_err()
    );
    for value in [false, true] {
        assert_eq!(Watch::decode(&Watch(value).encode()), Some(Watch(value)));
    }
    assert_eq!(Watch::decode(&[]), None);
    round::<MissionSnapshot>(
        json!({"missions":[{"slug":"demo","id":"m","title":"Demo","rev":1,"steps":3}]}),
    );
    round::<RunsSnapshot>(json!([]));
}

#[test]
fn rejected_start_keeps_every_engine_finding() {
    let value = json!({"error":{"code":"run_admission_failed","message":"run admission failed"},"findings":[{"code":"missing_input","nodeId":"input","message":"required"},{"code":"missing_file","nodeId":"step","message":"missing","path":"brief.md"}]});
    round::<AdmissionRejection>(value);
}

#[test]
fn remaining_envelopes_and_queries_round_trip() {
    round::<RunsQuery>(json!({"mission":"demo","needs":"you"}));
    round::<AttemptsQuery>(json!({"step":"f"}));
    round::<ErrorResponse>(json!({"error":{"code":"not_found","message":"missing"}}));
    round::<EditLayout>(json!({"arrange":true}));
    round::<EditLayout>(json!({"edges":[{"id":"e","lane":"top"}]}));
    round::<LayoutResponse>(
        json!({"layout":{"schema":1,"missionId":"m","missionRev":1,"nodes":[],"edges":[],"etag":"e"}}),
    );
    round::<GateRequestResponse>(
        json!({"request":{"gateId":"g","requestedSeq":2,"criterion":"Ready?","input":{"text":"yes","truncated":false}}}),
    );
    round::<AttemptsResponse>(json!({"attempts":{"runId":"r","steps":[]}}));
}

#[test]
fn notes_group_by_target_and_keep_order() {
    let entry = |id: &str, target: &str| NoteEntry {
        id: id.into(),
        target: target.into(),
        author: "human:test".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        text: id.into(),
        ..Default::default()
    };
    let notes = Notes {
        mission_id: "m".into(),
        entries: vec![entry("1", "f"), entry("2", "mission"), entry("3", "f")],
        ..Default::default()
    };
    let document = document::NotesDocument::from_notes(notes, "e".into());
    assert_eq!(document.mission[0].id, "2");
    assert_eq!(
        document.elements[0]
            .entries
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["1", "3"]
    );
    assert_eq!(document.updated_at, "0001-01-01T00:00:00Z");
}

#[test]
fn gate_profiles_keep_the_old_response() {
    let value: Value = serde_json::from_str(include_str!("fixtures/gate_profiles.json")).unwrap();
    round::<GateProfiles>(value);
}

#[test]
fn character_requests_and_responses_round_trip() {
    let character =
        json!({"id":"c","name":"Reviewer","agentId":"reviewer","harness":"codex","effort":"high"});
    round::<Character>(character.clone());
    round::<Characters>(json!({"characters":[character]}));
    round::<Deleted>(json!({"deleted":"c"}));
}
