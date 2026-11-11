//! A mission's data as it crosses the wire: the mission definition and its
//! nodes, its layout and notes, authoring requests and results, run
//! requests, a run's public projection, attempts and escalations, and
//! validation findings. It is plain data, so an area's contract may use it
//! (ARCHITECTURE.md, rule 3); the mission engine reads, writes, validates
//! and runs it.

mod authoring;
mod finding;
mod layout;
mod mission;
mod nodes;
mod notes;
mod run;
mod run_attempts;
mod run_projection;

pub use authoring::{
    Edit, EndUpdate, GateUpdate, InputCardUpdate, LayoutEdit, LayoutPatch, LimitUpdate,
    MissionDeletion, NotePatch, NotesEdit, Patch, Precondition, RestoredNode,
};
pub use finding::{Code, Finding, Report};
pub use layout::{LAYOUT_SCHEMA, Layout, LayoutEdge, LayoutNode};
pub use mission::{MISSION_SCHEMA, Mission, MissionListing};
pub use nodes::{
    Brief, Connection, DONE, END_IN, End, Formation, GATE_FAIL, GATE_IN, GATE_JUDGE, GATE_PASS,
    Gate, INPUT_CARD_OUT, InputCard, KIND_FILE, KIND_FOLDER, KIND_TEXT, Limit, MissionInput,
    MissionOutput, ORCHESTRATED, PEER, Port, REJECTED, SOLO, Slot,
};
pub use notes::{MISSION_THREAD, NOTES_SCHEMA, NoteEntry, Notes, valid_author};
pub use run::{
    HumanPick, HumanVerdict, ResumeMode, ResumeRequest, RunInput, RunOutput, RunSessionSummary,
    RunStart, RunSummary,
};
pub use run_attempts::{
    AttemptOutput, AttemptPick, AttemptVerdict, EvidenceItem, EvidenceText, RunAttempts,
    StepAttempt, StepAttempts, StepPort,
};
pub use run_projection::{
    Escalation, GateInput, GateRequest, PublicEvent, PublicInputRef, PublicOrigin, RunProjection,
    WaitingGate,
};
