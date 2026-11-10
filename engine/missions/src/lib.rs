//! Parvi's mission engine. This crate holds the mission model, the state
//! directory that stores missions, notes, layouts, role cards and
//! characters, and the validation that names what stops a run.
//!
//! Archon's contract is the behavioural spec (`docs/research/archon-to-parvi.md`):
//! its mission files load unchanged, and validation emits its finding codes.

mod arrange;
mod attempt_limits;
mod authoring;
mod authoring_controls;
mod authoring_create;
mod authoring_delete;
mod authoring_error;
mod authoring_input;
mod authoring_judge;
mod authoring_port;
mod authoring_restore;
mod authoring_staff;
mod authoring_type;
mod authoring_wire;
mod characters;
mod code_binding;
mod code_check;
mod contract_names;
mod contract_outputs;
mod contract_verdict;
mod coordinator;
mod error;
mod executor;
mod finding;
mod gate_replay;
mod gate_request;
mod graph;
mod harness;
mod human_replay;
mod kept_copy;
mod layout;
mod layout_authoring;
mod layout_placement;
mod ledger;
mod ledger_migrate;
mod limits;
mod links;
mod mission;
mod nodes;
mod notes;
mod notes_authoring;
mod output_refs;
pub mod outputs;
mod port_delivery;
mod roles;
mod roles_authoring;
mod run_admission;
mod run_attempts;
mod run_bindings;
mod run_capture;
mod run_dispatch;
mod run_escalation;
mod run_gate;
mod run_inputs;
mod run_job;
mod run_judges;
mod run_left_seats;
mod run_limit;
mod run_open_dispatch;
mod run_pick;
mod run_projection;
mod run_reattach;
mod run_recovery;
mod run_replay;
mod run_resume;
mod run_scheduler;
mod run_shutdown;
mod run_storage;
mod run_worker;
mod run_workspace;
mod run_workspace_recovery;
mod seats;
mod store;
mod turn_worktrees;
mod validate;

mod verdict;
mod workspaces;
mod write;

pub use arrange::arrange;
pub use attempt_limits::{AttemptLimits, TokenMeter};
pub use authoring::{Edit, MissionDeletion, Patch, Precondition};
pub use authoring_controls::{EndUpdate, GateUpdate, LimitUpdate};
pub use authoring_error::AuthoringError;
pub use authoring_input::InputCardUpdate;
pub use authoring_restore::RestoredNode;
pub use characters::{Character, CharacterStore};
pub use code_binding::CodeGateBinding;
pub use code_check::{CodeGateEvaluator, GateEvaluator, evaluate_code_check};
pub use contract_outputs::{OutputPayload, parse_outputs};
pub use contract_verdict::{JudgeVerdict, parse_verdict};
pub use coordinator::{
    AdmissionFailure, Coordinator, OnBlock, ResumeMode, ResumeRequest, RunSettings, RunStart,
    RunSummary,
};
pub use error::DefinitionError;
pub use executor::{
    ExecutionEvent, ExecutionSink, Executor, FormationExecution, FormationResult, JudgeCorrection,
    LabExecutor, LeftSeat, OpenDispatch, UnavailableExecutor,
};
pub use finding::{Code, Finding, Report};
pub use gate_request::{GateInput, GateRequest, GateRequestError};
pub use harness::{
    Claude, Codex, HarnessAdapter, Harnesses, Keys, Launch, Pi, Pointer, Shell, SlotBinding,
    TokenUsage, TranscriptRoots, Turn, TurnState, codex_threads,
};
pub use kept_copy::{Budget, KeptCopy, NotKept};
pub use layout::{Layout, LayoutEdge, LayoutNode};
pub use layout_authoring::{LayoutEdit, LayoutPatch};
pub use layout_placement::free_layout_position;
pub use ledger::{Ledger, RunEvent, validate as validate_ledger};
pub use limits::{LimitUse, LimitWarning};
pub use mission::Mission;
pub use nodes::{
    Brief, Connection, End, Formation, Gate, InputCard, Limit, MissionInput, MissionOutput, Port,
    Slot,
};
pub use notes::{MISSION_THREAD, NoteEntry, Notes};
pub use notes_authoring::{NotePatch, NotesEdit};
pub use port_delivery::{GateEvidence, GateFeedback, GateResponse, RoutedInput, merge_port_inputs};
pub use roles::{RoleCard, RoleNote, RoleStore, RoleUsage, RoleUse, Unreadable};
pub use roles_authoring::RoleEdit;
pub use run_admission::run_admission;
pub use run_attempts::{
    AttemptOutput, AttemptPick, AttemptVerdict, EvidenceItem, EvidenceText, RunAttempts,
    StepAttempt, StepAttempts, StepPort,
};
pub use run_bindings::{BindingsSnapshot, RunBinding};
pub use run_dispatch::{CompletionSentinel, parse_completion_sentinel};
pub use run_escalation::{Escalation, EscalationSentinel, parse_escalation, project_escalations};
pub use run_inputs::{
    RunInput, WORK_REFERENCES, input_findings, mission_run_inputs, render_run_inputs,
    resolve_run_inputs, substitute_run_inputs, work_placeholder,
};
pub use run_pick::HumanPick;
pub use run_projection::{PublicEvent, PublicInputRef, PublicOrigin, RunProjection, WaitingGate};
pub use seats::{
    AgentSeat, Dispatch, DispatchId, DispatchResult, LocalSeats, SeatEvent, SeatExecutor,
    SeatFuture, SeatId, SeatRecords, SeatSide, SeatWait, Submission, SubmissionAction, TurnWatch,
    stage_pointer,
};
pub use store::{MissionListing, Store};
pub use turn_worktrees::{PublishedTurn, TurnWorkspace, TurnWorktrees};
pub use validate::validate;
pub use verdict::HumanVerdict;
pub use workspaces::{Workspaces, Worktree};

/// Who acted when a request names no one: Parvi itself, as Archon's
/// `agent:archon` named Archon.
pub const DEFAULT_ACTOR: &str = "agent:parvi";
