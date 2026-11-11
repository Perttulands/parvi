//! Public, sanitized views of a validated run ledger.
use crate::{RunEvent, RunInput, validate_ledger};
use serde::Serialize;
use std::io;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicEvent {
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub slot_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gate_id: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub attempt: i64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub requested_seq: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub commit: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub verdict: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session_name: String,
    /// On `seat_created`: the node the seat runs on and its tmux session
    /// id, which with the name tell this seat from a later namesake.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub seat_node: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub since: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub end_ids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub input_refs: Vec<PublicInputRef>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicInputRef {
    pub edge_id: String,
    pub from_node_id: String,
    pub from_port_id: String,
    pub to_port_id: String,
    pub origin: Option<PublicOrigin>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
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
#[derive(Clone, Debug, Serialize)]
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
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunProjection {
    pub run_id: String,
    pub status: String,
    pub r#final: bool,
    pub resume_allowed: bool,
    pub epoch: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub resume_policy: String,
    #[serde(skip_serializing_if = "String::is_empty")]
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
    #[serde(skip_serializing_if = "String::is_empty")]
    pub request_id: String,
    pub context_paths: Vec<String>,
    pub inputs: Vec<RunInput>,
    pub outputs: Vec<crate::outputs::RunOutput>,
    pub event_count: usize,
    pub started_by: String,
    pub started_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ended_by: String,
    pub waiting_gates: Vec<WaitingGate>,
    pub events: Vec<PublicEvent>,
}
fn string(event: &RunEvent, name: &str) -> String {
    event
        .data
        .get(name)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned()
}
fn strings(event: &RunEvent, name: &str) -> Vec<String> {
    event
        .data
        .get(name)
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}
impl RunProjection {
    pub fn from_events(events: &[RunEvent], run_id: &str) -> Result<Self, crate::Error> {
        (|| -> io::Result<Self> {
            validate_ledger(events, run_id).map_err(crate::error::to_io)?;
            let first = &events[0];
            let mut result = Self {
                run_id: run_id.to_owned(),
                status: "running".into(),
                r#final: false,
                resume_allowed: false,
                epoch: 0,
                resume_policy: String::new(),
                block_code: String::new(),
                mission_slug: string(first, "missionSlug"),
                mission_id: first.mission_id.clone(),
                mission_rev: first.mission_rev,
                input_card_id: first.input_card_id.clone(),
                work: first.work.clone(),
                cwd: string(first, "cwd"),
                repository: string(first, "repository"),
                base: string(first, "base"),
                request_id: string(first, "requestId"),
                context_paths: strings(first, "contextPaths"),
                inputs: first
                    .data
                    .get("inputs")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default(),
                outputs: Vec::new(),
                event_count: events.len(),
                started_by: first.actor.clone(),
                started_at: first.timestamp.clone(),
                updated_at: first.timestamp.clone(),
                ended_by: String::new(),
                waiting_gates: Vec::new(),
                events: Vec::new(),
            };
            for event in events {
                result.updated_at.clone_from(&event.timestamp);
                result.epoch = event.epoch;
                match event.kind.as_str() {
                    "run_started" | "run_resumed" => {
                        result.status = "running".into();
                        result.r#final = false;
                        result.resume_allowed = false;
                        result.block_code.clear();
                        result.resume_policy.clear();
                    }
                    "run_blocked" => {
                        result.status = "blocked".into();
                        result.block_code = string(event, "code");
                        result.resume_policy = string(event, "resumePolicy");
                        result.resume_allowed = event
                            .data
                            .get("resumeAllowed")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                    }
                    "run_succeeded" | "run_failed" | "run_picked" | "run_canceled" => {
                        result.outputs = event
                            .data
                            .get("outputs")
                            .and_then(|v| serde_json::from_value(v.clone()).ok())
                            .unwrap_or_default();
                        result.status = event.kind.trim_start_matches("run_").into();
                        result.r#final = true;
                        result.resume_allowed = false;
                        result.block_code.clear();
                        result.resume_policy.clear();
                        if result.status == "failed" || result.status == "canceled" {
                            result.ended_by.clone_from(&event.actor);
                        }
                    }
                    "gate_evaluating" => {
                        result
                            .waiting_gates
                            .retain(|gate| gate.gate_id != event.gate_id);
                    }
                    "human_input_requested" => {
                        result
                            .waiting_gates
                            .retain(|gate| gate.gate_id != event.gate_id);
                        result.waiting_gates.push(WaitingGate {
                            gate_id: event.gate_id.clone(),
                            requested_seq: event.seq,
                            requested_at: event.timestamp.clone(),
                            pick: event.data.get("pick").cloned(),
                        });
                    }
                    "human_verdict_recorded" => {
                        let requested = event
                            .data
                            .get("requestedSeq")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        result.waiting_gates.retain(|gate| {
                            gate.gate_id != event.gate_id
                                || (requested != 0 && gate.requested_seq != requested)
                        });
                    }
                    _ => {}
                }
                result.events.push(PublicEvent {
                    seq: event.seq,
                    kind: event.kind.clone(),
                    node_id: event.node_id.clone(),
                    slot_id: event.slot_id.clone(),
                    gate_id: event.gate_id.clone(),
                    attempt: event.attempt,
                    requested_seq: event
                        .data
                        .get("requestedSeq")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    status: string(event, "status"),
                    commit: string(event, "commit"),
                    verdict: string(event, "verdict"),
                    outcome: string(event, "outcome"),
                    code: string(event, "code"),
                    session_name: string(event, "sessionName"),
                    seat_node: if event.kind == "seat_created" {
                        string(event, "node")
                    } else {
                        String::new()
                    },
                    session_id: if event.kind == "seat_created" {
                        string(event, "sessionId")
                    } else {
                        String::new()
                    },
                    state: string(event, "state"),
                    detail: string(event, "detail"),
                    since: string(event, "since"),
                    end_ids: strings(event, "endIds"),
                    input_refs: event
                        .data
                        .get("inputRefs")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                        .map(|v| PublicInputRef {
                            edge_id: v["edgeId"].as_str().unwrap_or("").into(),
                            from_node_id: v["fromNodeId"].as_str().unwrap_or("").into(),
                            from_port_id: v["fromPortId"].as_str().unwrap_or("").into(),
                            to_port_id: v["toPortId"].as_str().unwrap_or("").into(),
                            origin: serde_json::from_value(v["origin"].clone()).ok(),
                        })
                        .collect(),
                });
            }
            if result.r#final {
                result.waiting_gates.clear();
            } else if result.status != "blocked" && !result.waiting_gates.is_empty() {
                result.status = "waiting_human".into();
            }
            Ok(result)
        })()
        .map_err(crate::error::io)
    }
}
