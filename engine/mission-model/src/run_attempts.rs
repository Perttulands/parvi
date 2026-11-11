//! A run's attempts (Archon's run attempts evidence): for each step, every
//! attempt with what it output, the verdicts that judged it and the pick
//! that took it.

use serde::{Deserialize, Serialize};

/// One served text's cap (Archon's `EvidenceTextMaxBytes`).
const TEXT_MAX: usize = 64 << 10;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct EvidenceText {
    pub text: String,
    pub bytes: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl EvidenceText {
    /// The text as served: capped at a run's served text size.
    pub fn of(text: &str) -> Self {
        let mut t = Self {
            text: text.to_owned(),
            bytes: text.len(),
            truncated: false,
        };
        t.cap(TEXT_MAX);
        t
    }
    /// Cuts at a character boundary so the text stays valid UTF-8.
    pub fn cap(&mut self, max: usize) {
        if self.text.len() > max {
            let end = self.text.floor_char_boundary(max);
            self.text.truncate(end);
            self.truncated = true;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunAttempts {
    pub run_id: String,
    pub steps: Vec<StepAttempts>,
}

/// One step and its attempts, oldest first.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StepAttempts {
    pub node_id: String,
    pub title: String,
    /// The gates this step judges for, when it is a judge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub judge_of: Vec<String>,
    pub outputs: Vec<StepPort>,
    pub attempts: Vec<StepAttempt>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StepPort {
    pub id: String,
    pub label: String,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StepAttempt {
    pub attempt: i64,
    /// `running`, `done` or `stopped`.
    pub status: String,
    pub started_seq: u64,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ended_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<EvidenceText>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub output_seq: u64,
    pub outputs: Vec<AttemptOutput>,
    pub verdicts: Vec<AttemptVerdict>,
    /// The pick that took this attempt as its step's best.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picked: Option<AttemptPick>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// One output port's accepted payload; `path` is a file or folder output's
/// absolute path.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttemptOutput {
    pub port_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub kind: String,
    pub text: EvidenceText,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
}

/// One gate evaluation of the attempt's output and the verdict it reached,
/// or `pending` while it waits. `superseded` marks a human request a later
/// evaluation replaced before anyone answered it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttemptVerdict {
    pub gate_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gate_title: String,
    pub evaluation_seq: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seq: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub verdict: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub superseded: bool,
    pub reason: EvidenceText,
    pub evidence: Vec<EvidenceItem>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub decided_by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EvidenceItem {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    pub text: EvidenceText,
}

/// The pick that took an attempt: its gate, the verdict's seq, why and who,
/// the attempts it chose from and what started it (`whenSpent` or `resume`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttemptPick {
    pub gate_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gate_title: String,
    pub seq: u64,
    pub reason: EvidenceText,
    pub decided_by: String,
    pub candidates: Vec<i64>,
    pub trigger: String,
}
