//! The scheduled area's wire: tasks that paste a prompt into named sessions
//! on a schedule (#189), CHROTE's scheduler ported. The API is CHROTE's,
//! envelope, headers and task document all, so the `scheduled-tasks` skill
//! works against a node by changing only its base URL. The node and the app
//! both use these types, so a changed field fails the build on the other
//! side.
//!
//! Errors are the one shape `parvi_core::Error` defines, with the codes
//! below; over HTTP they come in CHROTE's envelope (see [`Envelope`]).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Watch task changes while the Scheduled page is shown.
pub const WATCH: u8 = 0xc8;
/// The node's tasks changed; the page should read them again.
pub const CHANGED: u8 = 0xc0;
pub const KINDS: parvi_core::frame::Kinds = &[WATCH..=WATCH];

/// The tasks' HTTP root: `GET` lists, `POST` creates; `/{id}` reads,
/// patches and deletes; `POST /{id}/pause`, `/resume` and `/run-now` act.
pub const PATH: &str = "/api/scheduled-tasks";

/// Every mutation carries this header with [`INTENT_VALUE`], CHROTE's guard
/// against a page that posts by accident; a create or patch is also JSON.
pub const INTENT: &str = "x-chrote-intent";
pub const INTENT_VALUE: &str = "scheduled-task";

/// The task does not exist.
pub const NOT_FOUND: &str = "not_found";
/// A request that cannot be kept: a bad task, schedule or JSON.
pub const BAD_REQUEST: &str = "bad_request";
/// A target names a session this node does not have.
pub const TARGET_NOT_FOUND: &str = "target_not_found";
/// The task is being modified or run by another call.
pub const CONFLICT: &str = "conflict";
/// A mutation without [`INTENT`].
pub const FORBIDDEN: &str = "forbidden";
/// A create or patch that is not JSON.
pub const UNSUPPORTED_MEDIA_TYPE: &str = "unsupported_media_type";
/// The node's own failure: a file that cannot be written.
pub const INTERNAL: &str = "internal";

/// What an HTTP route answers: `{success, data, timestamp}`, or
/// `{success: false, error, timestamp}` with the error in the one shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<parvi_core::Error>,
    pub timestamp: String,
}

pub const SUCCESS: &str = "success";
pub const ERROR: &str = "error";
/// Some targets took the prompt and others did not.
pub const PARTIAL: &str = "partial";
/// The receipt a guarded submit stores; it does not claim the program took
/// the prompt.
pub const SUBMIT_KEY_DISPATCHED: &str =
    "pasted; submit key dispatched (application acceptance unconfirmed)";
const LEGACY_SUBMITTED: &str = "pasted and submitted";

/// A session by its Unix user and name; sockets are the node's business.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub session_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unix_user: String,
}

impl Target {
    pub fn label(&self) -> String {
        if self.unix_user.is_empty() {
            self.session_name.clone()
        } else {
            format!("{}/{}", self.unix_user, self.session_name)
        }
    }
}

/// An interval or cron trigger, CHROTE's schema.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expression: String,
    #[serde(default)]
    pub timezone: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub every_minutes: i64,
    /// An interval as a Go duration (`24h`, `1h30m`), as CHROTE's files
    /// may have it instead of `everyMinutes`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub duration: String,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// One target's part of a run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "TargetRunDoc")]
pub struct TargetRun {
    pub session_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unix_user: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pane: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub submit_key_dispatched: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}

/// What older CHROTE builds wrote: `submitted`, which claimed too much, is
/// read as the transport receipt it was.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetRunDoc {
    session_name: String,
    #[serde(default)]
    unix_user: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    pane: String,
    #[serde(default)]
    submit_key_dispatched: bool,
    #[serde(default)]
    submitted: bool,
    #[serde(default)]
    message: String,
}

impl From<TargetRunDoc> for TargetRun {
    fn from(d: TargetRunDoc) -> TargetRun {
        TargetRun {
            session_name: d.session_name,
            unix_user: d.unix_user,
            status: d.status,
            pane: d.pane,
            submit_key_dispatched: d.submit_key_dispatched || d.submitted,
            message: if d.message == LEGACY_SUBMITTED {
                SUBMIT_KEY_DISPATCHED.into()
            } else {
                d.message
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEntry {
    pub id: String,
    pub trigger: String,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<TargetRun>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}

/// The persisted task document, CHROTE's schema field for field, so the
/// `scheduled-tasks` skill and CHROTE's own task files work unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "TaskDoc")]
pub struct Task {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub targets: Vec<Target>,
    pub schedule: Schedule,
    pub enabled: bool,
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub updated_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_runs: Vec<RunEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audit: Vec<AuditEntry>,
    /// Why the node holds the task back and will not fire it (its run could
    /// not be written ahead or recorded, it was mid-run when the node
    /// stopped, or its file stopped reading) until it is edited or resumed.
    /// Not CHROTE's; said by the API, never written to the file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub held: String,
    /// A run begun and not yet recorded, written before anything is sent:
    /// found on a task as the node starts, it means the node stopped
    /// mid-run, and the task is held rather than sent again. Not CHROTE's;
    /// CHROTE ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<Attempt>,
}

impl Task {
    /// Whether the schedule is keeping it.
    pub fn live(&self) -> bool {
        self.enabled && !self.paused
    }
}

/// A run in progress, as written ahead of its delivery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub run_id: String,
    pub trigger: String,
    pub started_at: Timestamp,
}

/// A task file as CHROTE wrote it over time: the retired single `target`
/// folds into `targets`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskDoc {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    targets: Vec<Target>,
    #[serde(default)]
    target: Option<Target>,
    #[serde(default)]
    schedule: Schedule,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    paused: bool,
    next_run: Option<Timestamp>,
    last_run: Option<Timestamp>,
    #[serde(default)]
    last_status: String,
    #[serde(default)]
    created_by: String,
    #[serde(default)]
    updated_by: String,
    #[serde(default = "epoch")]
    created_at: Timestamp,
    #[serde(default = "epoch")]
    updated_at: Timestamp,
    #[serde(default)]
    recent_runs: Vec<RunEntry>,
    #[serde(default)]
    audit: Vec<AuditEntry>,
    #[serde(default)]
    attempt: Option<Attempt>,
}

fn epoch() -> Timestamp {
    Timestamp::UNIX_EPOCH
}

impl From<TaskDoc> for Task {
    fn from(d: TaskDoc) -> Task {
        Task {
            id: d.id,
            name: d.name,
            prompt: d.prompt,
            targets: normalize_targets(d.targets.into_iter().chain(d.target)),
            schedule: d.schedule,
            enabled: d.enabled,
            paused: d.paused,
            next_run: d.next_run,
            last_run: d.last_run,
            last_status: d.last_status,
            created_by: d.created_by,
            updated_by: d.updated_by,
            created_at: d.created_at,
            updated_at: d.updated_at,
            recent_runs: d.recent_runs,
            audit: d.audit,
            held: String::new(),
            attempt: d.attempt,
        }
    }
}

/// Trimmed, without empties or repeats, in the caller's order.
pub fn normalize_targets(targets: impl IntoIterator<Item = Target>) -> Vec<Target> {
    let mut out: Vec<Target> = vec![];
    for t in targets {
        let t = Target {
            session_name: t.session_name.trim().into(),
            unix_user: t.unix_user.trim().into(),
        };
        if !t.session_name.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// A target as a request names it. `socket` is refused: the node resolves
/// sockets itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetRequest {
    #[serde(default)]
    pub session_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unix_user: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub socket: String,
}

/// `POST /api/scheduled-tasks`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub prompt: String,
    /// The retired single target, refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<serde_json::Value>,
    #[serde(default)]
    pub targets: Vec<TargetRequest>,
    #[serde(default)]
    pub schedule: Schedule,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub updated_by: String,
}

/// `PATCH /api/scheduled-tasks/{id}`: what is absent stays.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The retired single target, refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<TargetRequest>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub updated_by: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CHROTE's TestTargetRunJSONMigratesLegacySubmissionClaim.
    #[test]
    fn target_run_json_migrates_legacy_submission_claim() {
        let run: TargetRun = serde_json::from_str(
            r#"{"sessionName":"ops","status":"success","submitted":true,"message":"pasted and submitted"}"#,
        )
        .unwrap();
        assert!(run.submit_key_dispatched);
        assert_eq!(run.message, SUBMIT_KEY_DISPATCHED);
        let back = serde_json::to_string(&run).unwrap();
        assert!(!back.contains("\"submitted\""), "{back}");
        assert!(back.contains("\"submitKeyDispatched\":true"));
    }

    /// CHROTE's TestStoreReadsLegacySingleTargetDocument and
    /// TestTaskJSONWritesCurrentTargetsSchema.
    #[test]
    fn reads_a_legacy_single_target_and_writes_targets() {
        let task: Task = serde_json::from_str(
            r#"{"id":"tsk_legacy","name":"n","prompt":"p","target":{"sessionName":" ops ","unixUser":"alice"},
                "schedule":{"type":"interval","everyMinutes":5,"timezone":"UTC"},"enabled":true,"paused":false,
                "createdAt":"2026-06-27T14:00:00Z","updatedAt":"2026-06-27T14:00:00.123456789Z"}"#,
        )
        .unwrap();
        assert_eq!(
            task.targets,
            [Target {
                session_name: "ops".into(),
                unix_user: "alice".into()
            }]
        );
        let back = serde_json::to_string(&task).unwrap();
        assert!(back.contains("\"targets\":[{\"sessionName\":\"ops\",\"unixUser\":\"alice\"}]"));
        assert!(!back.contains("\"target\":"), "{back}");
    }
}
