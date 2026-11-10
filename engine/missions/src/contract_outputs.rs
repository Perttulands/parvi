//! Parsing the named output block shared by the lab and seat executors.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct OutputPayload {
    pub text: String,
    pub r#ref: String,
    pub report_ref: String,
    pub artifact_ref: String,
    pub kind: String,
}

fn repair_wraps(text: &str) -> String {
    let mut result = String::new();
    let (mut inside, mut escaped, mut skip_indent) = (false, false, false);
    for ch in text.chars() {
        if skip_indent && (ch == ' ' || ch == '\t') {
            continue;
        }
        skip_indent = false;
        if inside && ch == '\n' {
            skip_indent = true;
            continue;
        }
        result.push(ch);
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            inside = !inside;
        }
    }
    result
}
fn decode(text: &str) -> Result<BTreeMap<String, OutputPayload>, String> {
    let raw: BTreeMap<String, Value> = serde_json::from_str(text)
        .or_else(|_| serde_json::from_str(&repair_wraps(text)))
        .map_err(|e| format!("invalid output JSON: {e}"))?;
    Ok(raw
        .into_iter()
        .map(|(key, value)| {
            let field = |name: &str| {
                value
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };
            (
                key,
                OutputPayload {
                    text: field("text"),
                    r#ref: field("ref"),
                    report_ref: field("reportRef"),
                    artifact_ref: field("artifactRef"),
                    kind: field("kind"),
                },
            )
        })
        .collect())
}

/// Return display text and the routed payloads. Port validation and ref
/// materialization are performed by the caller against its formation.
pub fn parse_outputs(text: &str) -> Result<(String, BTreeMap<String, OutputPayload>), String> {
    let marker = format!("```{}", crate::contract_names::OUTPUTS);
    if let Some(start) = text.rfind(&marker) {
        let body = text[start..]
            .find('\n')
            .map(|n| start + n + 1)
            .ok_or_else(|| "output fence is missing JSON body".to_string())?;
        let end = text[body..]
            .find("```")
            .map(|n| body + n)
            .ok_or_else(|| "output fence is missing closing fence".to_string())?;
        let outputs = decode(text[body..end].trim())?;
        if outputs.is_empty() {
            return Err("output block must contain at least one payload".into());
        }
        return Ok((
            format!("{}{}", &text[..start], &text[end + 3..])
                .trim()
                .into(),
            outputs,
        ));
    }
    let trimmed = text.trim();
    for (start, _) in trimmed.match_indices('{') {
        if let Ok(outputs) = decode(&trimmed[start..])
            && outputs.keys().any(|key| key.starts_with("port_"))
            && !outputs.is_empty()
        {
            return Ok((trimmed[..start].trim().into(), outputs));
        }
    }
    Ok((trimmed.into(), BTreeMap::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ports_fenced_bare_and_terminal_wrapped_payloads() {
        let (clean, result) =
            parse_outputs("hello\n```parvi-outputs\n{\"out\":{\"text\":\"ok\"}}\n```\nend")
                .unwrap();
        assert_eq!(clean, "hello\n\nend");
        assert_eq!(result["out"].text, "ok");
        let (clean, output) =
            parse_outputs("audit summary\n{\"port_report\":{\"text\":\"routed\"}}").unwrap();
        assert_eq!(clean, "audit summary");
        assert_eq!(output["port_report"].text, "routed");
        assert!(
            parse_outputs("audit\n{\"text\":\"not a port\"}")
                .unwrap()
                .1
                .is_empty()
        );
        let (_, output) =
            parse_outputs("{\"port_report\":{\"ref\":\"/tmp/long/\n path.md\"}}").unwrap();
        assert_eq!(output["port_report"].r#ref, "/tmp/long/path.md");
        assert!(parse_outputs("```parvi-outputs\n{}\n```").is_err());
    }
}
