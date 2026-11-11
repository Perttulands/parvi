//! Completion sentinels refer to one run; the latest matching one wins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionSentinel {
    pub run_id: String,
    pub status: String,
    pub artifact: String,
}
pub fn parse_completion_sentinel(captured: &str, run_id: &str) -> Option<CompletionSentinel> {
    let mut remaining = captured;
    let mut found = None;
    while let Some((_, tail)) = remaining.split_once(crate::contract_names::DONE) {
        let Some((raw, after)) = tail.split_once(">>>") else {
            break;
        };
        remaining = after;
        let fields: std::collections::BTreeMap<_, _> = raw
            .split_whitespace()
            .filter_map(|part| part.split_once('='))
            .map(|(k, v)| (k, v.trim_matches('"')))
            .collect();
        if fields.get("run-id").copied() != Some(run_id) {
            continue;
        }
        found = Some(CompletionSentinel {
            run_id: run_id.into(),
            status: fields.get("status").copied().unwrap_or("").into(),
            artifact: fields.get("artifact").copied().unwrap_or("").into(),
        });
    }
    found
}
#[cfg(test)]
mod tests {
    use super::parse_completion_sentinel;
    #[test]
    fn accepts_only_latest_sentinel_for_the_run() {
        let text = "<<<PARVI-DONE run-id=run_a status=ok artifact=first>>> <<<PARVI-DONE run-id=other status=ok artifact=wrong>>> <<<PARVI-DONE run-id=run_a status=ok artifact=second>>>";
        assert_eq!(
            parse_completion_sentinel(text, "run_a").unwrap().artifact,
            "second"
        );
        assert!(parse_completion_sentinel(text, "missing").is_none());
    }
}
