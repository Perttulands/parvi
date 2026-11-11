//! Validation findings: stable codes located on the mission.

use serde::{Deserialize, Serialize};

/// The code of a finding. Codes are Archon's, so its tools and skills read
/// Parvi's reports unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    DanglingConnection,
    DuplicateInputProducer,
    DuplicateNodeId,
    DuplicateSlotId,
    EmptyLimit,
    FormationWithoutSlots,
    GateNotRoutable,
    InvalidEnd,
    InvalidFormationShape,
    InvalidFormationType,
    InvalidHumanChannel,
    InvalidLimit,
    InvalidMissionInput,
    InvalidMissionOutput,
    InvalidPortKind,
    InvalidSlotId,
    InvalidSlotSettings,
    MissingFile,
    NoCheckout,
    PlacementWithoutRepository,
    MissingInput,
    MissionCount,
    MissionNotRunnable,
    MissionOutputKindMismatch,
    OrchestratedController,
    RelativeFile,
    RetiredPersona,
    RouteLeadsNowhere,
    SeveralInputCards,
    UnavailablePersona,
    UnboundMissionOutput,
    UnknownDevice,
    UnknownInputReference,
    UnknownInput,
    InvalidInput,
    UnreachableNode,
    UnstaffedSlot,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::DanglingConnection => "dangling_connection",
            Code::DuplicateInputProducer => "duplicate_input_producer",
            Code::DuplicateNodeId => "duplicate_node_id",
            Code::DuplicateSlotId => "duplicate_slot_id",
            Code::EmptyLimit => "empty_limit",
            Code::FormationWithoutSlots => "formation_without_slots",
            Code::GateNotRoutable => "gate_not_routable",
            Code::InvalidEnd => "invalid_end",
            Code::InvalidFormationShape => "invalid_formation_shape",
            Code::InvalidFormationType => "invalid_formation_type",
            Code::InvalidHumanChannel => "invalid_human_channel",
            Code::InvalidLimit => "invalid_limit",
            Code::InvalidMissionInput => "invalid_mission_input",
            Code::InvalidMissionOutput => "invalid_mission_output",
            Code::InvalidPortKind => "invalid_port_kind",
            Code::InvalidSlotId => "invalid_slot_id",
            Code::InvalidSlotSettings => "invalid_slot_settings",
            Code::MissingFile => "missing_file",
            Code::NoCheckout => "no_checkout",
            Code::PlacementWithoutRepository => "placement_without_repository",
            Code::MissingInput => "missing_input",
            Code::MissionCount => "mission_count",
            Code::MissionNotRunnable => "mission_not_runnable",
            Code::MissionOutputKindMismatch => "mission_output_kind_mismatch",
            Code::OrchestratedController => "orchestrated_controller",
            Code::RelativeFile => "relative_file",
            Code::RetiredPersona => "retired_persona",
            Code::RouteLeadsNowhere => "route_leads_nowhere",
            Code::SeveralInputCards => "several_input_cards",
            Code::UnavailablePersona => "unavailable_persona",
            Code::UnboundMissionOutput => "unbound_mission_output",
            Code::UnknownDevice => "unknown_device",
            Code::UnknownInputReference => "unknown_input_reference",
            Code::UnknownInput => "unknown_input",
            Code::InvalidInput => "invalid_input",
            Code::UnreachableNode => "unreachable_node",
            Code::UnstaffedSlot => "unstaffed_slot",
        }
    }
}

impl Serialize for Code {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// One problem, located on the node it is about: a node id, a connection id,
/// or "" for the mission itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub code: Code,
    pub node_id: String,
    pub message: String,
    /// The reference file a `missing_file` or `relative_file` finding names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl Finding {
    pub fn new(code: Code, node_id: &str, message: String) -> Finding {
        Finding {
            code,
            node_id: node_id.to_owned(),
            message,
            path: None,
        }
    }
}

/// Every problem that would stop a run, and advice that would not.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub errors: Vec<Finding>,
    pub warnings: Vec<Finding>,
}

impl Report {
    /// Orders findings by code, then node, then message, so reports compare.
    pub fn sort(&mut self) {
        for findings in [&mut self.errors, &mut self.warnings] {
            findings.sort_by(|a, b| {
                (a.code.as_str(), &a.node_id, &a.message).cmp(&(
                    b.code.as_str(),
                    &b.node_id,
                    &b.message,
                ))
            });
        }
    }
}
