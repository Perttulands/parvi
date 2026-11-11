//! Authoring requests and results: one edit of a mission, its layout or its
//! notes, the precondition it is checked against, and what it answers.

use crate::{
    End, Formation, Gate, InputCard, Layout, LayoutEdge, LayoutNode, Limit, Mission, MissionInput,
    MissionOutput, Notes,
};

#[derive(Clone, Debug, Default)]
pub struct Precondition {
    pub rev: i64,
    pub etag: String,
}

#[derive(Clone, Debug)]
pub struct Edit {
    pub mission: Mission,
    pub etag: String,
}

/// A single board mutation. Further operations share the same checked write path.
#[derive(Clone, Debug)]
pub enum Patch {
    Title(String),
    SetGateJudge {
        gate_id: String,
        chain: Vec<String>,
    },
    DetachGateJudge {
        gate_id: String,
    },
    CreateLimit {
        limit: crate::Limit,
        x: i64,
        y: i64,
    },
    UpdateLimit(crate::LimitUpdate),
    UpdateEnd(crate::EndUpdate),
    UpdateGate(crate::GateUpdate),
    RestoreNode {
        node: crate::RestoredNode,
        connections: Vec<crate::Connection>,
        index: Option<usize>,
        x: i64,
        y: i64,
    },
    AddPort {
        formation_id: String,
        direction: String,
        label: String,
        kind: String,
    },
    UpdatePort {
        formation_id: String,
        port_id: String,
        label: Option<String>,
        kind: Option<String>,
    },
    RemovePort {
        formation_id: String,
        port_id: String,
    },
    RestorePort {
        formation_id: String,
        direction: String,
        port: crate::Port,
        index: Option<usize>,
        connections: Vec<crate::Connection>,
    },
    UpdateInputCard(crate::InputCardUpdate),
    SetBrief {
        formation_id: String,
        brief: crate::Brief,
    },
    ClearBrief {
        formation_id: String,
    },
    UpdateFormation {
        id: String,
        title: Option<String>,
        /// Where the step runs (#124); blank runs it on the coordinator.
        device: Option<String>,
    },
    EditFormation {
        id: String,
        title: String,
        brief: Option<crate::Brief>,
    },
    AssignSlot {
        formation_id: String,
        slot_id: String,
        agent_id: String,
        harness: String,
        model: String,
        effort: String,
    },
    SwapSlots {
        from_formation: String,
        from_slot: String,
        to_formation: String,
        to_slot: String,
    },
    MakeController {
        formation_id: String,
        slot_id: String,
    },
    SetFormationType {
        id: String,
        kind: String,
        keep_slot_id: String,
        slots: Option<Vec<crate::Slot>>,
    },
    WireConnection {
        id: String,
        from: String,
        to: String,
        join_if_occupied: bool,
    },
    RewireConnection {
        from: String,
        previous_to: String,
        to: String,
        join_if_occupied: bool,
        remove_previous_input: bool,
    },
    UnwireConnection {
        from: String,
        to: String,
    },
    DeleteInputCard {
        id: String,
    },
    DeleteFormation {
        id: String,
    },
    DeleteGate {
        id: String,
    },
    DeleteEnd {
        id: String,
    },
    DeleteLimit {
        id: String,
    },
    CreateInputCard {
        title: String,
        goal: String,
        files: Vec<String>,
        human_channel: String,
        x: i64,
        y: i64,
    },
    CreateFormation {
        kind: String,
        title: String,
        x: i64,
        y: i64,
    },
    CreateGate {
        title: String,
        kinds: Vec<String>,
        criterion: String,
        check: String,
        check_version: String,
        check_value: String,
        files: Vec<String>,
        x: i64,
        y: i64,
    },
    CreateEnd {
        title: String,
        outcome: String,
        x: i64,
        y: i64,
    },
}

/// An archived mission and the id shared by its archived layout.
#[derive(Clone, Debug)]
pub struct MissionDeletion {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub archive_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct EndUpdate {
    pub id: String,
    pub title: Option<String>,
    pub outcome: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct LimitUpdate {
    pub id: String,
    pub title: Option<String>,
    pub target: Option<String>,
    pub rounds: Option<i64>,
    pub seconds: Option<i64>,
    pub warn_seconds: Option<i64>,
    pub tokens: Option<i64>,
    pub when_spent: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct GateUpdate {
    pub id: String,
    pub title: Option<String>,
    pub kinds: Option<Vec<String>>,
    pub criterion: Option<String>,
    pub check: Option<String>,
    pub check_version: Option<String>,
    pub check_value: Option<String>,
    pub files: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default)]
pub struct InputCardUpdate {
    pub id: String,
    pub title: Option<String>,
    pub goal: Option<String>,
    pub files: Option<Vec<String>>,
    pub input_hint: Option<String>,
    pub human_channel: Option<String>,
    pub inputs: Option<Vec<MissionInput>>,
    pub outputs: Option<Vec<MissionOutput>>,
}

#[derive(Clone, Debug)]
pub enum RestoredNode {
    InputCard(InputCard),
    Formation(Formation),
    Gate(Gate),
    End(End),
    Limit(Limit),
}

#[derive(Clone, Debug)]
pub enum LayoutPatch {
    Nodes(Vec<LayoutNode>),
    Edges(Vec<LayoutEdge>),
}

#[derive(Clone, Debug)]
pub struct LayoutEdit {
    pub layout: Layout,
    pub etag: String,
}

#[derive(Clone, Debug)]
pub enum NotePatch {
    Append {
        target: String,
        author: String,
        text: String,
    },
    Edit {
        target: String,
        author: String,
        id: String,
        text: String,
    },
    Delete {
        target: String,
        author: String,
        id: String,
    },
}

#[derive(Clone, Debug)]
pub struct NotesEdit {
    pub notes: Notes,
    pub etag: String,
}
