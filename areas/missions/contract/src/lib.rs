//! Mission wire types shared by the node and its clients. This crate does no I/O.

use serde::{Deserialize, Serialize};

pub mod patch;
pub use parvi_mission_model::*;

pub const WATCH: u8 = 0xa8;
pub const WATCH_RUNS: u8 = 0xa9;
pub const SNAPSHOT: u8 = 0xa0;
pub const RUNS_SNAPSHOT: u8 = 0xa1;
pub const KINDS: parvi_core::frame::Kinds = &[0xa0..=0xa1, 0xa8..=0xa9];

pub mod error {
    pub const BAD_REQUEST: &str = "bad_request";
    pub const NOT_FOUND: &str = "not_found";
    pub const CONFLICT: &str = "conflict";
    pub const RUN_ADMISSION_FAILED: &str = "run_admission_failed";
    pub const RUN_ERROR: &str = "run_error";
    pub const RUN_FINAL: &str = "run_final";
    pub const NOT_PENDING: &str = "not_pending";
    pub const PRECONDITION_REQUIRED: &str = "precondition_required";
    pub const MISSION_EXISTS: &str = "mission_exists";
    pub const CHARACTER_EXISTS: &str = "character_exists";
    pub const INVALID_CHARACTER: &str = "invalid_character";
    pub const ALREADY_EXISTS: &str = "already_exists";
    pub const DUPLICATE_CONNECTION: &str = "duplicate_connection";
    pub const INPUT_OCCUPIED: &str = "input_occupied";
    pub const INVALID_BEAD_ID: &str = "invalid_bead_id";
    pub const INVALID_CODE_GATE_PROFILE: &str = "invalid_code_gate_profile";
    pub const INVALID_CONTROLLER_ROLE: &str = "invalid_controller_role";
    pub const INVALID_DEFINITION: &str = "invalid_definition";
    pub const INVALID_END_OUTCOME: &str = "invalid_end_outcome";
    pub const INVALID_GATE_KIND: &str = "invalid_gate_kind";
    pub const INVALID_HUMAN_CHANNEL: &str = "invalid_human_channel";
    pub const INVALID_LIMIT: &str = "invalid_limit";
    pub const INVALID_MISSION_INPUT: &str = "invalid_mission_input";
    pub const INVALID_MISSION_OUTPUT: &str = "invalid_mission_output";
    pub const INVALID_NODE_RESTORE: &str = "invalid_node_restore";
    pub const INVALID_NOTE_PATCH: &str = "invalid_note_patch";
    pub const INVALID_PORT: &str = "invalid_port";
    pub const INVALID_PORT_DIRECTION: &str = "invalid_port_direction";
    pub const INVALID_SLOT_SETTINGS: &str = "invalid_slot_settings";
    pub const INVALID_TYPE_CHANGE: &str = "invalid_type_change";
    pub const IO: &str = "io";
    pub const NOTE_AUTHOR_MISMATCH: &str = "note_author_mismatch";
    pub const NOTE_TOO_LARGE: &str = "note_too_large";
    pub const RELATIVE_FILE_REFERENCE: &str = "relative_file_reference";
    pub const SELF_WIRE: &str = "self_wire";
    pub const SLOT_CHOICE_REQUIRED: &str = "slot_choice_required";
    pub const UNSUPPORTED_FORMATION_TYPE: &str = "unsupported_formation_type";
    pub const UNSUPPORTED_SCHEMA: &str = "unsupported_schema";
    pub const INVALID_DEFINITION_SOURCE: &str = "invalid_definition_source";
}

/// The node maps an admission failure to a core error and, for a rejected
/// admission, includes every engine finding alongside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdmissionRejection {
    pub error: parvi_core::Error,
    pub findings: Vec<Finding>,
}

/// A watch frame carries exactly one byte; zero turns the subscription off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watch(pub bool);
impl Watch {
    pub fn encode(self) -> [u8; 1] {
        [u8::from(self.0)]
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [value] => Some(Self(*value != 0)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MissionSnapshot {
    pub missions: Vec<MissionListing>,
}
/// Both the initial snapshot and incremental updates use this payload.
pub type RunsSnapshot = Vec<RunProjection>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CreateMission {
    pub title: String,
    pub slug: String,
    pub updated_by: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DeleteMission {
    pub expected_rev: i64,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EditNotes {
    pub target: String,
    pub author: String,
    pub text: String,
    pub entry_id: String,
    pub action: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EditLayout {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arrange: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<LayoutNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edges: Option<Vec<LayoutEdge>>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Abort {
    pub requested_by: String,
    pub reason: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Land {
    pub force: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunId {
    pub run_id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Landed {
    pub landed: bool,
    pub notes: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSeat {
    pub dispatch_id: String,
    pub slot_id: String,
    pub node: String,
    pub session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane: Option<u32>,
    pub session_name: String,
    pub outcome: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting: Option<SeatWait>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeatWait {
    pub state: String,
    pub detail: String,
    pub since: String,
    pub seq: u64,
}

pub mod document;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MissionResponse {
    pub mission: document::MissionDocument,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutResponse {
    pub layout: document::LayoutDocument,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NotesResponse {
    pub notes: document::NotesDocument,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationResponse {
    pub mission_rev: i64,
    pub mission_etag: String,
    pub errors: Vec<Finding>,
    pub warnings: Vec<Finding>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deletion {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub archive_id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeletionResponse {
    pub deletion: Deletion,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttemptsResponse {
    pub attempts: RunAttempts,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateRequestResponse {
    pub request: GateRequest,
}
/// Additional fields name the node created or deleted by this particular edit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditResponse {
    pub mission: document::MissionDocument,
    pub layout: document::LayoutDocument,
    #[serde(flatten)]
    pub changed: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunsQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AttemptsQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: parvi_core::Error,
}

/// Describes the built-in, deterministic checks offered when authoring a gate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateProfiles {
    pub profiles: Vec<GateProfile>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateProfile {
    pub profile_id: String,
    pub profile_version: String,
    pub display_name: String,
    pub parameter_name: String,
    pub parameter_label: String,
    pub execution_class: String,
    pub effect_policy: String,
    pub deterministic: bool,
    pub certified_total: bool,
    pub max_input_bytes: u64,
    pub max_result_bytes: u64,
    pub max_operations: u64,
    pub result_encoding: String,
    pub profile_sha256: String,
    pub evaluator_bundle_sha256: String,
    pub policy_sha256: String,
    pub determinism_policy_sha256: String,
}

/// A saved staffing stamp; the engine's persisted character is mapped at the node boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Character {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub picture: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    pub harness: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    pub effort: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Characters {
    pub characters: Vec<Character>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deleted {
    pub deleted: String,
}
