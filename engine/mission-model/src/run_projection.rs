//! Public, sanitized views of a run: its projection, events, escalations
//! and a pending human gate's request. The engine projects them from the
//! run's private ledger.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::RunInput;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicEvent {
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slot_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gate_id: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attempt: i64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub requested_seq: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub verdict: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_name: String,
    /// On `seat_created`: the node the seat runs on and its tmux session
    /// id, which with the name tell this seat from a later namesake.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub seat_node: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub since: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub end_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_refs: Vec<PublicInputRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicInputRef {
    pub edge_id: String,
    pub from_node_id: String,
    pub from_port_id: String,
    pub to_port_id: String,
    pub origin: Option<PublicOrigin>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicOrigin {
    pub node_id: String,
    pub port_id: String,
    pub attempt: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingGate {
    pub gate_id: String,
    pub requested_seq: u64,
    pub requested_at: String,
    /// A pick request's candidates and the judge's recommendation: it is
    /// answered with a pick (`gates/{gate}/pick`), never a verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pick: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunProjection {
    pub run_id: String,
    pub status: String,
    pub r#final: bool,
    pub resume_allowed: bool,
    pub epoch: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resume_policy: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub block_code: String,
    pub mission_slug: String,
    pub mission_id: String,
    pub mission_rev: i64,
    pub input_card_id: String,
    /// The run's work item: a canonical reference, or empty.
    pub work: String,
    pub cwd: String,
    pub repository: String,
    pub base: String,
    /// The client's key for the start that made this run, or empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_id: String,
    pub context_paths: Vec<String>,
    pub inputs: Vec<RunInput>,
    pub outputs: Vec<crate::RunOutput>,
    pub event_count: usize,
    pub started_by: String,
    pub started_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ended_by: String,
    pub waiting_gates: Vec<WaitingGate>,
    pub events: Vec<PublicEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Escalation {
    pub run_id: String,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gate_id: String,
    pub severity: String,
    pub reason: String,
    pub source: String,
    pub trigger: String,
    pub blocks: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GateInput {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from_node_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from_port_id: String,
    pub text: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GateRequest {
    pub gate_id: String,
    pub requested_seq: u64,
    /// The gate's criterion as the run froze it.
    pub criterion: String,
    pub input: GateInput,
    /// A pick request's candidates and the judge's recommendation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pick: Option<Value>,
}
