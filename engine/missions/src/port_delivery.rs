//! Unread work and human or gate annotations carried by a formation port.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GateEvidence {
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GateFeedback {
    pub gate_id: String,
    pub gate_attempt: i64,
    pub verdict: String,
    pub reason: String,
    pub evidence: Vec<GateEvidence>,
    pub original_ref: String,
    pub original_text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub original_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub earlier: Option<Box<GateFeedback>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GateResponse {
    pub gate_id: String,
    pub gate_attempt: i64,
    pub requested_seq: u64,
    pub decided_by: String,
    pub text: String,
    pub original_ref: String,
    pub original_text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub original_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub earlier: Option<Box<GateResponse>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RoutedInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<Box<GateFeedback>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<Box<GateResponse>>,
    pub edge_id: String,
    pub from_node_id: String,
    pub from_port_id: String,
    pub to_port_id: String,
    pub r#ref: String,
    pub text: String,
    pub report_ref: String,
    pub artifact_ref: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<crate::PublicOrigin>,
}

fn chain_feedback(
    newest: Option<Box<GateFeedback>>,
    earlier: Option<Box<GateFeedback>>,
) -> Option<Box<GateFeedback>> {
    match newest {
        None => earlier,
        Some(mut value) => {
            value.earlier = chain_feedback(value.earlier.take(), earlier);
            Some(value)
        }
    }
}
fn chain_response(
    newest: Option<Box<GateResponse>>,
    earlier: Option<Box<GateResponse>>,
) -> Option<Box<GateResponse>> {
    match newest {
        None => earlier,
        Some(mut value) => {
            value.earlier = chain_response(value.earlier.take(), earlier);
            Some(value)
        }
    }
}

/// Merge two deliveries that have not yet been read. A send-back keeps any
/// unread work, while all annotations travel newest first.
pub fn merge_port_inputs(mut older: RoutedInput, mut newer: RoutedInput) -> RoutedInput {
    let keep_older_work = newer.feedback.is_some() && older.feedback.is_none();
    let feedback = chain_feedback(newer.feedback.take(), older.feedback.take());
    let response = chain_response(newer.response.take(), older.response.take());
    let mut result = if keep_older_work { older } else { newer };
    result.feedback = feedback;
    result.response = response;
    result
}

pub(crate) fn render_inputs(inputs: &[RoutedInput]) -> String {
    use std::fmt::Write;
    let mut text = String::new();
    for input in inputs {
        if !input.text.is_empty()
            && input
                .feedback
                .as_ref()
                .is_none_or(|f| f.original_text != input.text)
        {
            let _ = writeln!(text, "input: {}", input.text);
        }
        let mut feedback = input.feedback.as_deref();
        while let Some(item) = feedback {
            let _ = write!(
                text,
                "\ngate feedback from {}, attempt {}:\nverdict: {}\nreason: {}\n",
                item.gate_id, item.gate_attempt, item.verdict, item.reason
            );
            for evidence in &item.evidence {
                let _ = writeln!(text, "evidence: {}", evidence.text);
            }
            let _ = write!(
                text,
                "original input ref: {}\noriginal input: {}\n\n",
                item.original_ref, item.original_text
            );
            feedback = item.earlier.as_deref();
        }
        let mut response = input.response.as_deref();
        while let Some(item) = response {
            let _ = write!(
                text,
                "\nhuman response from {}, attempt {}:\nverdict: pass\ndecided by: {}\nresponse:\n{}\napproved input ref: {}\napproved input: {}\n\n",
                item.gate_id,
                item.gate_attempt,
                item.decided_by,
                item.text,
                item.original_ref,
                item.original_text
            );
            response = item.earlier.as_deref();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn send_back_keeps_unread_work_and_chains_annotations() {
        let old = RoutedInput {
            text: "new work".into(),
            response: Some(Box::new(GateResponse {
                text: "first".into(),
                ..Default::default()
            })),
            ..Default::default()
        };
        let sent_back = RoutedInput {
            text: "older work".into(),
            feedback: Some(Box::new(GateFeedback {
                reason: "fix".into(),
                ..Default::default()
            })),
            response: Some(Box::new(GateResponse {
                text: "second".into(),
                ..Default::default()
            })),
            ..Default::default()
        };
        let result = merge_port_inputs(old, sent_back);
        assert_eq!(result.text, "new work");
        assert_eq!(result.feedback.unwrap().reason, "fix");
        let response = result.response.unwrap();
        assert_eq!(response.text, "second");
        assert_eq!(response.earlier.unwrap().text, "first");
    }
    #[test]
    fn newer_work_keeps_each_response_bound_to_what_was_approved() {
        let older = RoutedInput {
            text: "old work".into(),
            response: Some(Box::new(GateResponse {
                text: "first answer".into(),
                original_text: "old work".into(),
                ..Default::default()
            })),
            ..Default::default()
        };
        let newer = RoutedInput {
            text: "new work".into(),
            response: Some(Box::new(GateResponse {
                text: "second answer".into(),
                original_text: "new work".into(),
                ..Default::default()
            })),
            ..Default::default()
        };
        let merged = merge_port_inputs(older, newer);
        assert_eq!(merged.text, "new work");
        let response = merged.response.unwrap();
        assert_eq!(response.original_text, "new work");
        assert_eq!(response.earlier.unwrap().original_text, "old work");
    }
}
