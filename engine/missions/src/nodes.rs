//! The nodes of a mission graph and the wires between them.

use serde::{Deserialize, Serialize};

/// What a mission input, mission output or output port carries.
pub const KIND_TEXT: &str = "text";
pub const KIND_FILE: &str = "file";
pub const KIND_FOLDER: &str = "folder";

/// The mission's entry node (`[[inputCard]]`), with its `out` port.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct InputCard {
    pub id: String,
    pub title: String,
    pub goal: String,
    /// Reference files, as absolute paths.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// What a run's brief should hold, when the mission declares no inputs.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub input_hint: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<MissionInput>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<MissionOutput>,
    /// `notify` (the default, also blank) or `session`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub human_channel: String,
}

/// A named value each run supplies. Briefs reference it as `{name}`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MissionInput {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// `text` (also blank), `file` or `folder`.
    pub kind: String,
    pub required: bool,
}

/// A named result a successful run promises, bound to one step output port.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MissionOutput {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// `text` (also blank), `file` or `folder`.
    pub kind: String,
    pub required: bool,
    /// The bound step output port, `<step-id>:<port-id>`; blank is a draft.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from: String,
}

/// A step: the team of agents that does it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Formation {
    pub id: String,
    /// `solo`, `peer` or `orchestrated`.
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    /// The environment the step runs on; blank runs it on the coordinator.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub device: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brief: Option<Brief>,
    #[serde(rename = "input")]
    pub inputs: Vec<Port>,
    #[serde(rename = "output")]
    pub outputs: Vec<Port>,
    #[serde(rename = "slot")]
    pub slots: Vec<Slot>,
}

pub const SOLO: &str = "solo";
pub const PEER: &str = "peer";
pub const ORCHESTRATED: &str = "orchestrated";

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Brief {
    pub goal: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub repository: String,
    /// The work item this step serves, when it differs from the run's: a
    /// reference in any written form (`docs/work.md`, References).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub work: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Port {
    pub id: String,
    pub label: String,
    /// What an output port carries: `text` (also absent), `file` or
    /// `folder`. Input ports carry no kind.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

impl Port {
    /// The port's kind, `text` when unset.
    pub fn kind(&self) -> &str {
        match self.kind.trim() {
            "" => KIND_TEXT,
            kind => kind,
        }
    }

    /// The port's label, or its id when it has none.
    pub fn name(&self) -> &str {
        match self.label.trim() {
            "" => &self.id,
            _ => &self.label,
        }
    }
}

/// A position in a formation, owning what its seat runs.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Slot {
    pub id: String,
    pub label: String,
    /// The slot's optional role card; none is a vanilla agent.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub harness: String,
    /// Blank is the harness default model.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    pub controller: bool,
}

impl Slot {
    /// Whether the slot names anything to run: a harness, model, effort or role.
    pub fn staffed(&self) -> bool {
        [&self.agent_id, &self.harness, &self.model, &self.effort]
            .iter()
            .any(|value| !value.is_empty())
    }

    /// The slot as sentences name it: `"Worker" (w1)`, or `"w1"`.
    pub fn name(&self) -> String {
        if self.label.is_empty() {
            format!("{:?}", self.id)
        } else {
            format!("{:?} ({})", self.label, self.id)
        }
    }
}

/// A criterion with one or more kinds. Its ports are `in`, `pass`, `fail`
/// and `judge`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Gate {
    pub id: String,
    pub title: String,
    /// `code`, `formation` or `human`.
    pub kinds: Vec<String>,
    /// Free-text intent, never run as a step.
    pub criterion: String,
    /// A code check's profile, its exact version and its parameter.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub check: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub check_version: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub check_value: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

pub const GATE_IN: &str = "in";
pub const GATE_PASS: &str = "pass";
pub const GATE_FAIL: &str = "fail";
pub const GATE_JUDGE: &str = "judge";

/// Ends a path on purpose. Its only port is `in`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct End {
    pub id: String,
    pub title: String,
    /// `done` or `rejected`.
    pub outcome: String,
}

pub const END_IN: &str = "in";
pub const DONE: &str = "done";
pub const REJECTED: &str = "rejected";

/// Caps the rounds, time and tokens of one step, or of the whole mission when
/// it covers the Input card. A knob is `None` when the card does not set it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Limit {
    pub id: String,
    pub title: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rounds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warn_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i64>,
    /// `stop` (also blank), `best` or `fail`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub when_spent: String,
}

/// A directed wire between `node-id:port-id` endpoints.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Connection {
    pub id: String,
    pub from: String,
    pub to: String,
}

/// An Input card's only port.
pub const INPUT_CARD_OUT: &str = "out";
