//! Escalation evidence projected separately from the sanitized run view.
use crate::RunEvent;
use parvi_mission_model::Escalation;
pub fn project_escalations(events: &[RunEvent]) -> Vec<Escalation> {
    events
        .iter()
        .filter(|e| e.kind == "escalation_raised")
        .map(|e| {
            let text = |key: &str| {
                e.data
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned()
            };
            Escalation {
                run_id: e.run_id.clone(),
                seq: e.seq,
                node_id: e.node_id.clone(),
                gate_id: e.gate_id.clone(),
                severity: text("severity"),
                reason: text("reason"),
                source: text("source"),
                trigger: text("trigger"),
                blocks: e
                    .data
                    .get("blocks")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscalationSentinel {
    pub run_id: String,
    pub severity: String,
    pub reason: String,
    pub node_id: String,
    pub gate_id: String,
    pub blocks: bool,
}
/// Read only a sentinel for this run, including quoted reasons with spaces.
pub fn parse_escalation(captured: &str, run_id: &str) -> Option<EscalationSentinel> {
    let mut remaining = captured;
    while let Some((_, tail)) = remaining.split_once(crate::contract_names::ESCALATE) {
        let (raw, after) = tail.split_once(">>>")?;
        remaining = after;
        let fields = sentinel_fields(raw);
        if fields.get("run-id").map(String::as_str) != Some(run_id) {
            continue;
        }
        let severity = fields
            .get("severity")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| "needs-attention".into());
        let blocks = fields.get("blocks").is_some_and(|v| v == "true") || severity == "stop";
        return Some(EscalationSentinel {
            run_id: run_id.into(),
            reason: fields.get("reason").cloned().unwrap_or_default(),
            node_id: fields
                .get("node")
                .or_else(|| fields.get("nodeId"))
                .cloned()
                .unwrap_or_default(),
            gate_id: fields.get("gateId").cloned().unwrap_or_default(),
            severity,
            blocks,
        });
    }
    None
}
fn sentinel_fields(raw: &str) -> std::collections::BTreeMap<String, String> {
    let bytes = raw.as_bytes();
    let mut fields = std::collections::BTreeMap::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b' ' {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            while i < bytes.len() && bytes[i] != b' ' {
                i += 1;
            }
            continue;
        }
        let key = &raw[start..i];
        i += 1;
        let value = if i >= bytes.len() {
            ""
        } else if bytes[i] == b'\'' || bytes[i] == b'"' {
            let quote = bytes[i];
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            let value = &raw[start..i];
            if i < bytes.len() {
                i += 1;
            }
            value
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b' ' {
                i += 1;
            }
            &raw[start..i]
        };
        fields.insert(key.into(), value.into());
    }
    fields
}
#[cfg(test)]
mod tests {
    use super::parse_escalation;
    #[test]
    fn finds_matching_run_and_quoted_reason() {
        let captured = "<<<PARVI-ESCALATE run-id=other reason=ignored>>> <<<PARVI-ESCALATE run-id=run_1 severity=stop reason='needs å review' nodeId=step>>>";
        let found = parse_escalation(captured, "run_1").unwrap();
        assert_eq!(found.reason, "needs å review");
        assert_eq!(found.node_id, "step");
        assert!(found.blocks);
        assert!(parse_escalation(captured, "run_2").is_none());
    }
}
