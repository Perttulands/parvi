//! A pending human gate's request (Archon's `gate_request.go`): what the gate
//! asks, and the output of the step that feeds it, so the answer window can
//! quote it. Never refs, paths, prompts or session identities.
use crate::{Coordinator, RunEvent, RunProjection};
use serde::Serialize;
use serde_json::Value;
use std::io;

/// How much of the input is served; the full output stays in the run's
/// evidence (Archon's `EvidenceTextMaxBytes`).
const INPUT_MAX: usize = 64 << 10;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GateInput {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from_node_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from_port_id: String,
    pub text: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
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

/// Why a gate's request cannot be served.
#[derive(Debug)]
pub enum GateRequestError {
    /// No run, or the gate never asked a human.
    NotFound(String),
    /// The gate's latest request was answered or replaced.
    NotPending,
    Io(io::Error),
}

impl From<io::Error> for GateRequestError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::NotFound {
            Self::NotFound(e.to_string())
        } else {
            Self::Io(e)
        }
    }
}

/// The gate's latest human request while it is pending.
pub fn pending_request(
    run: &str,
    gate: &str,
    events: &[RunEvent],
    criterion: &str,
) -> Result<GateRequest, GateRequestError> {
    let status = RunProjection::from_events(events, run)?;
    let request = (events.iter().rev())
        .find(|e| e.kind == "human_input_requested" && e.gate_id == gate)
        .ok_or_else(|| GateRequestError::NotFound("human gate request not found".into()))?;
    let pending =
        (status.waiting_gates.iter()).any(|g| g.gate_id == gate && g.requested_seq == request.seq);
    if !pending {
        return Err(GateRequestError::NotPending);
    }
    let input = request.data.get("inputRef");
    let field = |name: &str| {
        input
            .and_then(|i| i.get(name))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let mut text = field("text");
    let truncated = text.len() > INPUT_MAX;
    if truncated {
        text.truncate(text.floor_char_boundary(INPUT_MAX));
    }
    Ok(GateRequest {
        gate_id: gate.to_owned(),
        requested_seq: request.seq,
        criterion: criterion.to_owned(),
        input: GateInput {
            from_node_id: field("fromNodeId"),
            from_port_id: field("fromPortId"),
            text,
            truncated,
        },
        pick: request.data.get("pick").cloned(),
    })
}

impl Coordinator {
    /// A pending human gate's request on run `id`.
    pub fn gate_request(&self, id: &str, gate: &str) -> Result<GateRequest, GateRequestError> {
        let events = self.ledger(id)?.read()?;
        let mission = self.frozen_mission(id)?;
        let criterion = (mission.gates.iter())
            .find(|g| g.id == gate)
            .map(|g| g.criterion.clone())
            .unwrap_or_default();
        pending_request(id, gate, &events, &criterion)
    }
}
