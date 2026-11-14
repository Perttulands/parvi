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

/// A stop's reason as a person reads it: the codes a program matches on
/// (`seat_error: …`, `… (executor_failed)`) taken off, the sentence kept.
/// An agent's own answer reads as its words before its outputs block and
/// done line, first paragraph only.
pub fn plain_reason(detail: &str) -> String {
    let words = agent_words(detail);
    let detail = words.as_deref().unwrap_or(detail);
    let code = |word: &str| {
        // A code may name what it is about: `invalid_judge_result[gate]`.
        let word = match word.split_once('[') {
            Some((word, rest)) if rest.ends_with(']') => word,
            _ => word,
        };
        !word.is_empty()
            && word.contains('_')
            && word.chars().all(|c| c.is_ascii_lowercase() || c == '_')
    };
    // An id a person never sees: `fmn_6d758e032bde0bb7`.
    let id = |word: &str| {
        word.split_once('_').is_some_and(|(kind, rest)| {
            !kind.is_empty()
                && kind.chars().all(|c| c.is_ascii_lowercase())
                && rest.len() >= 6
                && rest.chars().any(|c| c.is_ascii_digit())
                && rest
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
    };
    let mut text = detail.trim();
    loop {
        let before = text;
        // Older ledgers put the engine's words in front of the reason.
        if let Some(rest) = text.strip_prefix("dispatch reattach failed: ") {
            text = rest.trim_start();
        }
        if let Some((head, rest)) = text.split_once(": ")
            && (code(head) || id(head))
        {
            text = rest.trim_start();
        }
        if let Some(open) = text.rfind(" (")
            && text.ends_with(')')
            && code(&text[open + 2..text.len() - 1])
        {
            text = text[..open].trim_end();
        }
        if text == before {
            break;
        }
    }
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The words of an agent's answer that ended with a done line, or `None`
/// when `detail` is not such an answer. An agent that wrote nothing before
/// its outputs is read by the first text it put out.
fn agent_words(detail: &str) -> Option<String> {
    let done = detail.find("<<<PARVI-DONE")?;
    let fence = detail
        .find("```parvi-outputs")
        .filter(|&fence| fence < done);
    let words = detail[..fence.unwrap_or(done)].trim();
    let first = words.split("\n\n").next().unwrap_or(words).trim();
    if !first.is_empty() {
        return Some(first.to_owned());
    }
    let put_out = fence.and_then(|fence| {
        let block = &detail[fence + "```parvi-outputs".len()..done];
        let block = block.split("```").next().unwrap_or(block);
        let outputs: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(block.trim()).ok()?;
        outputs.values().find_map(|output| {
            let text = output.get("text")?.as_str()?.lines().next()?.trim();
            (!text.is_empty()).then(|| text.to_owned())
        })
    });
    Some(put_out.unwrap_or_else(|| "the agent ended without saying why".to_owned()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_agents_answer_reads_as_its_first_words() {
        let answer = "The input file is missing, so I did nothing.\n\n```parvi-outputs\n{\"p\":{\"text\":\"x\"}}\n```\n\n<<<PARVI-DONE run-id=r status=blocked artifact=none>>>";
        assert_eq!(
            super::plain_reason(answer),
            "The input file is missing, so I did nothing."
        );
        let silent = "```parvi-outputs\n{\"p\":{\"text\":\"blocked: the poem needs a reader.\"}}\n```\n\n<<<PARVI-DONE run-id=r status=blocked artifact=p>>>.";
        assert_eq!(
            super::plain_reason(silent),
            "Blocked: the poem needs a reader."
        );
    }

    #[test]
    fn a_reason_reads_without_ids_or_engine_words() {
        assert_eq!(
            super::plain_reason(
                "dispatch reattach failed: fmn_6d758e032bde0bb7: the agent's session ended before it finished"
            ),
            "The agent's session ended before it finished"
        );
        assert_eq!(
            super::plain_reason("fmn_work: kept as written"),
            "Kept as written"
        );
    }

    #[test]
    fn a_reason_reads_without_its_codes() {
        assert_eq!(
            super::plain_reason("seat_idle_without_sentinel: the agent stopped (seat_error)"),
            "The agent stopped"
        );
        assert_eq!(
            super::plain_reason(
                "seat_error: seat_stuck: the agent did not take its brief. Its screen says: model: gone"
            ),
            "The agent did not take its brief. Its screen says: model: gone"
        );
        assert_eq!(
            super::plain_reason("invalid_judge_result[gate_review]: missing verdict block"),
            "Missing verdict block"
        );
        assert_eq!(super::plain_reason("escalation"), "Escalation");
    }
}
