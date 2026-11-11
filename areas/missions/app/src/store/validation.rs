//! Validation answers are usable only for the exact definition they describe.
use parvi_missions_contract::{Code, ValidationResponse};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub node: String,
    pub slug: String,
    pub rev: i64,
    pub etag: String,
}

#[derive(Default)]
pub struct Validation {
    key: Option<Key>,
    report: Option<Result<ValidationResponse, String>>,
    generation: u64,
}
impl Validation {
    /// Return true once per definition; callers may explicitly retry failures.
    pub fn begin(&mut self, key: Key) -> Option<u64> {
        if self.key.as_ref() == Some(&key) {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.key = Some(key);
        self.report = None;
        Some(self.generation)
    }
    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.key = None;
        self.report = None;
    }
    pub fn finish(
        &mut self,
        key: &Key,
        generation: u64,
        report: Result<ValidationResponse, String>,
    ) {
        if self.key.as_ref() != Some(key) || self.generation != generation {
            return;
        }
        self.report = Some(report.and_then(|report| {
            if report.mission_rev == key.rev && report.mission_etag == key.etag {
                Ok(report)
            } else {
                Err("The mission changed while validation was running.".into())
            }
        }));
    }
    pub fn complete(&self, key: &Key) -> bool {
        self.key.as_ref() == Some(key) && self.report.is_some()
    }
    pub fn failed(&self, key: &Key) -> bool {
        self.key.as_ref() == Some(key) && matches!(self.report, Some(Err(_)))
    }
    pub fn runnable(&self, key: &Key, input: &str) -> bool {
        if self.key.as_ref() != Some(key) {
            return false;
        }
        matches!(&self.report, Some(Ok(report)) if report.errors.is_empty() && !report.warnings.iter().any(|finding| finding.code == Code::MissionNotRunnable && finding.node_id == input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_missions_contract::Finding;
    fn key(rev: i64) -> Key {
        Key {
            node: "n".into(),
            slug: "m".into(),
            rev,
            etag: format!("v{rev}"),
        }
    }
    fn report(rev: i64) -> ValidationResponse {
        ValidationResponse {
            mission_rev: rev,
            mission_etag: format!("v{rev}"),
            errors: vec![],
            warnings: vec![],
        }
    }
    #[test]
    fn stale_replies_and_unwired_input_never_enable_run() {
        let (old, current) = (key(1), key(2));
        let mut state = Validation::default();
        let old_request = state.begin(old.clone()).unwrap();
        let current_request = state.begin(current.clone()).unwrap();
        state.finish(&old, old_request, Ok(report(1)));
        assert!(!state.runnable(&current, "input"));
        state.finish(&current, current_request, Ok(report(1)));
        assert!(state.failed(&current));
        state.clear();
        let new_request = state.begin(current.clone()).unwrap();
        state.finish(&current, current_request, Ok(report(2)));
        assert!(!state.runnable(&current, "input"));
        let mut result = report(2);
        result.warnings.push(Finding::new(
            Code::MissionNotRunnable,
            "input",
            "not connected".into(),
        ));
        state.finish(&current, new_request, Ok(result));
        assert!(!state.runnable(&current, "input"));
        assert!(state.runnable(&current, "other"));
    }
}
