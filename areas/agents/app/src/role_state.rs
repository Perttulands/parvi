pub use parvi_agents_contract::roles::{Unreadable, Usage};

/// The text fields accepted by the role API. Launch settings belong to slots.
pub fn role_change(name: &str, kind: &str, summary: &str, capabilities: &str) -> serde_json::Value {
    let caps: Vec<&str> = capabilities
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    serde_json::json!({"displayName": name, "kind": kind, "summary": summary, "capabilities": caps})
}

/// The UI offers deletion only after a complete scan finds no references.
pub fn may_delete(usage: Option<&Usage>) -> bool {
    usage.is_some_and(|u| u.complete && u.usage.is_empty())
}

/// Never imply a missing or incomplete scan proves that a card is unused.
pub fn usage_words(usage: Option<&Usage>) -> String {
    match usage {
        None => "Usage has not been read.".into(),
        Some(u) if !u.complete => format!("{} known uses; usage scan incomplete.", u.usage.len()),
        Some(u) if u.usage.is_empty() => "No mission slots use this role.".into(),
        Some(u) => format!("{} mission slots use this role.", u.usage.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_agents_contract::roles::Use;
    #[test]
    fn role_edits_contain_only_role_text() {
        let change = role_change("Writer", "specialist", "Writes", " draft, , review ");
        assert_eq!(
            change,
            serde_json::json!({"displayName":"Writer", "kind":"specialist", "summary":"Writes", "capabilities":["draft", "review"]})
        );
    }

    #[test]
    fn incomplete_scan_never_claims_the_role_is_unused() {
        let usage: Usage = serde_json::from_str(r#"{"usage":[],"complete":false,"unreadable":[{"name":"bad.toml","reason":"invalid"}]}"#).unwrap();
        assert_eq!(
            usage_words(Some(&usage)),
            "0 known uses; usage scan incomplete."
        );
        assert_eq!(usage.unreadable[0].name, "bad.toml");
        assert!(!may_delete(None));
        assert!(!may_delete(Some(&usage)));
        let mut complete = usage.clone();
        complete.complete = true;
        assert!(may_delete(Some(&complete)));
        complete.usage.push(Use::default());
        assert!(!may_delete(Some(&complete)));
        assert_eq!(usage_words(None), "Usage has not been read.");
    }
}
