//! Validation answers are usable only for the exact definition they describe.
use parvi_missions_contract::{Code, Mission, ValidationResponse};
use std::collections::HashMap;

/// Render validation prose using known titles and labels while retaining the
/// node IDs in the underlying report for actions and diagnostics.
/// One thing a mission still lacks, said as what to do: "Choose who works
/// Write", "Connect Input to a first step". Findings without a plainer
/// form keep the node's own words, with names for ids.
pub fn todo_line(mission: &Mission, finding: &parvi_missions_contract::Finding) -> String {
    let name = mission.node_name(&finding.node_id);
    let step = mission.formations.iter().any(|f| f.id == finding.node_id);
    match finding.code {
        Code::UnstaffedSlot | Code::FormationWithoutSlots => format!("Choose who works {name}"),
        Code::InvalidSlotSettings if finding.message.contains("needs an effort") => {
            format!("Choose an effort for {name}")
        }
        Code::RouteLeadsNowhere if step => format!("Connect {name} to a next step or an End"),
        Code::MissionNotRunnable if mission.has_input_card(&finding.node_id) => {
            format!("Connect {name} to a first step")
        }
        Code::UnreachableNode => format!("Connect {name} after the Input or an earlier step"),
        _ => readable_message(
            mission,
            &parvi_missions_contract::plain_reason(&finding.message),
        ),
    }
}

/// What a mission still lacks before it can run, one line per thing to do:
/// findings that ask for the same thing share a line, so the count is the
/// list's length.
pub fn todo_lines(
    mission: &Mission,
    blocking: impl IntoIterator<Item = parvi_missions_contract::Finding>,
) -> Vec<(String, Vec<parvi_missions_contract::Finding>)> {
    let mut lines: Vec<(String, Vec<parvi_missions_contract::Finding>)> = Vec::new();
    for finding in blocking {
        let finding = &finding;
        let line = todo_line(mission, finding);
        match lines.iter_mut().find(|(text, _)| *text == line) {
            Some((_, findings)) => findings.push(finding.clone()),
            None => lines.push((line, vec![finding.clone()])),
        }
    }
    lines
}

/// An error from the node as the editor says it: no codes, names instead
/// of ids, and the effort policy's long explanation cut to what to do.
pub fn plain_error(mission: Option<&Mission>, text: &str) -> String {
    let text = parvi_missions_contract::plain_reason(text);
    if text.contains("needs an effort") {
        return "Choose an effort for this agent.".into();
    }
    match mission {
        Some(mission) => readable_message(mission, &text),
        None => text,
    }
}

pub fn readable_message(mission: &Mission, message: &str) -> String {
    let mut names = HashMap::<&str, &str>::new();
    fn add<'a>(names: &mut HashMap<&'a str, &'a str>, id: &'a str, name: &'a str) {
        if !id.is_empty() && !name.trim().is_empty() {
            names.insert(id, name.trim());
        }
    }
    for node in &mission.input_cards {
        add(&mut names, &node.id, &node.title);
    }
    for node in &mission.formations {
        add(&mut names, &node.id, &node.title);
        for port in node.inputs.iter().chain(&node.outputs) {
            add(&mut names, &port.id, &port.label);
        }
        for slot in &node.slots {
            add(&mut names, &slot.id, &slot.label);
        }
    }
    for node in &mission.gates {
        add(&mut names, &node.id, &node.title);
    }
    for node in &mission.ends {
        add(&mut names, &node.id, &node.title);
    }
    for node in &mission.limits {
        add(&mut names, &node.id, &node.title);
    }
    for edge in &mission.connections {
        add(&mut names, &edge.id, "connection");
    }

    let mut out = String::with_capacity(message.len());
    let mut start = 0;
    let mut quoted = false;
    for (i, c) in message.char_indices() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            continue;
        }
        if start < i {
            let token = &message[start..i];
            let distinctive = token
                .bytes()
                .any(|b| b == b'_' || b == b'-' || b.is_ascii_digit());
            if let Some(name) = names.get(token).filter(|_| quoted || distinctive) {
                out.push_str(name);
            } else {
                out.push_str(token);
            }
        }
        out.push(c);
        if c == '"' {
            quoted = !quoted;
        }
        start = i + c.len_utf8();
    }
    if start < message.len() {
        let token = &message[start..];
        let distinctive = token
            .bytes()
            .any(|b| b == b'_' || b == b'-' || b.is_ascii_digit());
        out.push_str(
            names
                .get(token)
                .filter(|_| quoted || distinctive)
                .copied()
                .unwrap_or(token),
        );
    }
    out
}

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
    /// The report is available only for the exact selected definition.
    pub fn report(&self, key: &Key) -> Option<&ValidationResponse> {
        if self.key.as_ref() != Some(key) {
            return None;
        }
        self.report.as_ref()?.as_ref().ok()
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
    #[test]
    fn findings_use_known_names_without_replacing_parts_of_other_words() {
        let mission: Mission = toml_fixture();
        assert_eq!(
            readable_message(
                &mission,
                "Input card \"inp_1\" has no outgoing connection; step \"fmn_10\" uses \"slot_1\"; unknown inp_12"
            ),
            "Input card \"Start\" has no outgoing connection; step \"Build\" uses \"Worker\"; unknown inp_12"
        );
        assert_eq!(
            readable_message(
                &mission,
                "connection \"edge_1\" from \"inp_1:out\" to \"fmn_10:port_1\""
            ),
            "connection \"connection\" from \"Start:out\" to \"Build:Input\""
        );
    }
    #[test]
    fn the_to_do_says_each_thing_once_in_plain_words() {
        let mission: Mission = toml_fixture();
        let findings = [
            Finding::new(
                Code::UnstaffedSlot,
                "fmn_10",
                "slot \"slot_1\" is unstaffed".into(),
            ),
            Finding::new(
                Code::UnstaffedSlot,
                "fmn_10",
                "slot \"slot_2\" is unstaffed".into(),
            ),
            Finding::new(
                Code::InvalidSlotSettings,
                "fmn_10",
                "invalid_slot_settings: slot needs an effort; the policy is long".into(),
            ),
        ];
        let lines: Vec<_> = todo_lines(&mission, findings)
            .into_iter()
            .map(|(line, found)| (line, found.len()))
            .collect();
        assert_eq!(
            lines,
            [
                ("Choose who works Build".to_owned(), 2),
                ("Choose an effort for Build".to_owned(), 1)
            ]
        );
    }
    fn toml_fixture() -> Mission {
        serde_json::from_value(serde_json::json!({"inputCard":[{"id":"inp_1","title":"Start"}],"formation":[{"id":"fmn_10","title":"Build","input":[{"id":"port_1","label":"Input"}],"slot":[{"id":"slot_1","label":"Worker"}]}],"connection":[{"id":"edge_1","from":"inp_1:out","to":"fmn_10:port_1"}]})).unwrap()
    }
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
