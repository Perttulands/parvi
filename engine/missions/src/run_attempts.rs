//! A run's attempts (Archon's run attempts evidence, `run_evidence_attempts.go`):
//! for each step, every attempt in order with when it ran, what it output,
//! the gate verdicts that judged that output and the pick that took it. Each
//! gate evaluation names the output it judges (its inputRef's origin), so a
//! verdict joins the attempt it judged whichever route brought it there.
//!
//! Trimmed from Archon until the rest of the run evidence lands (#111): an
//! attempt's artifact directory listing; a file output carries its path,
//! read through Files.
use crate::{Mission, RunEvent, RunProjection};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

/// One served text's cap, and the whole answer's (Archon's
/// `EvidenceTextMaxBytes` and `EvidenceNodeBudgetBytes`).
const TEXT_MAX: usize = 64 << 10;
const BUDGET: usize = 2 << 20;
/// How many evidence items one verdict serves.
const ITEMS_MAX: usize = 100;

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct EvidenceText {
    pub text: String,
    pub bytes: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl EvidenceText {
    fn of(text: &str) -> Self {
        let mut t = Self {
            text: text.to_owned(),
            bytes: text.len(),
            truncated: false,
        };
        t.cap(TEXT_MAX);
        t
    }
    /// Cuts at a character boundary so the text stays valid UTF-8.
    fn cap(&mut self, max: usize) {
        if self.text.len() > max {
            let end = self.text.floor_char_boundary(max);
            self.text.truncate(end);
            self.truncated = true;
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunAttempts {
    pub run_id: String,
    pub steps: Vec<StepAttempts>,
}

/// One step and its attempts, oldest first.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StepAttempts {
    pub node_id: String,
    pub title: String,
    /// The gates this step judges for, when it is a judge.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub judge_of: Vec<String>,
    pub outputs: Vec<StepPort>,
    pub attempts: Vec<StepAttempt>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct StepPort {
    pub id: String,
    pub label: String,
    pub kind: String,
}

pub const RUNNING: &str = "running";
pub const DONE: &str = "done";
pub const STOPPED: &str = "stopped";

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StepAttempt {
    pub attempt: i64,
    /// `running`, `done` or `stopped`.
    pub status: String,
    pub started_seq: u64,
    pub started_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ended_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<EvidenceText>,
    #[serde(skip_serializing_if = "is_zero")]
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
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttemptOutput {
    pub port_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub kind: String,
    pub text: EvidenceText,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
}

/// One gate evaluation of the attempt's output and the verdict it reached,
/// or `pending` while it waits. `superseded` marks a human request a later
/// evaluation replaced before anyone answered it.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttemptVerdict {
    pub gate_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gate_title: String,
    pub evaluation_seq: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub seq: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub verdict: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub superseded: bool,
    pub reason: EvidenceText,
    pub evidence: Vec<EvidenceItem>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub decided_by: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct EvidenceItem {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    pub text: EvidenceText,
}

/// The pick that took an attempt: its gate, the verdict's seq, why and who,
/// the attempts it chose from and what started it (`whenSpent` or `resume`).
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttemptPick {
    pub gate_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gate_title: String,
    pub seq: u64,
    pub reason: EvidenceText,
    pub decided_by: String,
    pub candidates: Vec<i64>,
    pub trigger: String,
}

fn text(event: &RunEvent, name: &str) -> String {
    str_of(event.data.get(name))
}
fn str_of(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or("").to_owned()
}
fn origin(event: &RunEvent) -> Option<(String, i64)> {
    let origin = event.data.get("inputRef")?.get("origin")?;
    Some((
        str_of(origin.get("nodeId")),
        origin.get("attempt").and_then(Value::as_i64).unwrap_or(0),
    ))
}

/// Where one evaluation's verdict is: step, attempt and index.
type At = (String, i64, usize);

struct Walk<'a> {
    mission: &'a Mission,
    steps: Vec<StepAttempts>,
    index: HashMap<String, usize>,
    /// Each gate's latest evaluation still waiting on its verdict, and
    /// whether it asked a human.
    open: HashMap<String, (At, bool)>,
    /// Each human request's evaluation, by request seq.
    asked: HashMap<u64, At>,
    /// Steps a pick settled: a resume carries none of their attempts on.
    picking: HashMap<String, bool>,
}

impl Walk<'_> {
    fn current(&mut self, node: &str) -> Option<&mut StepAttempt> {
        let i = *self.index.get(node)?;
        self.steps[i].attempts.last_mut()
    }
    fn find(&mut self, node: &str, attempt: i64) -> Option<&mut StepAttempt> {
        let i = *self.index.get(node)?;
        self.steps[i]
            .attempts
            .iter_mut()
            .rev()
            .find(|a| a.attempt == attempt)
    }
    fn verdict(&mut self, at: &At) -> Option<&mut AttemptVerdict> {
        self.find(&at.0, at.1)?.verdicts.get_mut(at.2)
    }
    fn gate_title(&self, gate: &str) -> String {
        (self.mission.gates.iter())
            .find(|g| g.id == gate)
            .map(|g| g.title.clone())
            .unwrap_or_default()
    }

    fn started(&mut self, event: &RunEvent) {
        let Some(formation) = self.mission.formation(&event.node_id) else {
            return;
        };
        if let Some(previous) = self.current(&event.node_id)
            && previous.status == RUNNING
        {
            previous.status = STOPPED.into();
            if previous.ended_at.is_empty() {
                previous.ended_at.clone_from(&event.timestamp);
            }
        }
        if !self.index.contains_key(&event.node_id) {
            let judge_of = (self.mission.gates.iter())
                .filter(|g| {
                    crate::graph::judge_chain(self.mission, &g.id)
                        .iter()
                        .any(|f| f.id == formation.id)
                })
                .map(|g| g.id.clone())
                .collect();
            self.index.insert(event.node_id.clone(), self.steps.len());
            self.steps.push(StepAttempts {
                node_id: formation.id.clone(),
                title: formation.title.clone(),
                judge_of,
                outputs: (formation.outputs.iter())
                    .map(|p| StepPort {
                        id: p.id.clone(),
                        label: p.label.clone(),
                        kind: p.kind().to_owned(),
                    })
                    .collect(),
                attempts: Vec::new(),
            });
        }
        self.picking.remove(&event.node_id);
        let number = event.attempt.max(1);
        // A restart before dispatch records the same attempt again.
        if let Some(last) = self.current(&event.node_id)
            && last.attempt == number
            && last.output_seq == 0
        {
            last.status = RUNNING.into();
            last.ended_at.clear();
            last.stop_reason = None;
            return;
        }
        let i = self.index[&event.node_id];
        self.steps[i].attempts.push(StepAttempt {
            attempt: number,
            status: RUNNING.into(),
            started_seq: event.seq,
            started_at: event.timestamp.clone(),
            ended_at: String::new(),
            stop_reason: None,
            output_seq: 0,
            outputs: Vec::new(),
            verdicts: Vec::new(),
            picked: None,
        });
    }

    fn output(&mut self, event: &RunEvent) {
        let Some(&i) = self.index.get(&event.node_id) else {
            return;
        };
        let ports = self.steps[i].outputs.clone();
        let Some(attempt) = self.find(&event.node_id, event.attempt.max(1)) else {
            return;
        };
        attempt.status = DONE.into();
        attempt.output_seq = event.seq;
        attempt.ended_at.clone_from(&event.timestamp);
        let payloads = event.data.get("outputs").and_then(Value::as_object);
        let payload_of = |port: &str, label: &str, kind: &str| {
            let payload = payloads.and_then(|p| p.get(port))?;
            Some(AttemptOutput {
                port_id: port.to_owned(),
                label: label.to_owned(),
                kind: kind.to_owned(),
                text: EvidenceText::of(&str_of(payload.get("text"))),
                path: str_of(payload.get("ref")),
            })
        };
        // In the step's port order, then any port it no longer has.
        let mut outputs: Vec<_> = (ports.iter())
            .filter_map(|p| payload_of(&p.id, &p.label, &p.kind))
            .collect();
        for (port, payload) in payloads.into_iter().flatten() {
            if !ports.iter().any(|p| &p.id == port) {
                let kind = str_of(payload.get("kind"));
                let kind = if kind.is_empty() { "text" } else { &kind };
                outputs.extend(payload_of(port, "", kind));
            }
        }
        attempt.outputs = outputs;
    }

    fn evaluating(&mut self, event: &RunEvent, finished: bool) {
        let gate = event.gate_id.clone();
        // A new evaluation replaces a request still waiting.
        if let Some((at, asked)) = self.open.remove(&gate)
            && asked
            && let Some(replaced) = self.verdict(&at)
        {
            replaced.pending = false;
            replaced.superseded = true;
        }
        let Some((node, number)) = origin(event) else {
            return;
        };
        let gate_title = self.gate_title(&gate);
        let Some(attempt) = self.find(&node, number) else {
            return;
        };
        attempt.verdicts.push(AttemptVerdict {
            gate_id: gate.clone(),
            gate_title,
            evaluation_seq: event.seq,
            seq: 0,
            verdict: String::new(),
            pending: !finished,
            superseded: false,
            reason: EvidenceText::default(),
            evidence: Vec::new(),
            decided_by: String::new(),
        });
        let at = (node, number, attempt.verdicts.len() - 1);
        self.open.insert(gate, (at, false));
    }

    fn human_answer(&mut self, event: &RunEvent) {
        let requested = event.data.get("requestedSeq").and_then(Value::as_u64);
        let Some(at) = requested.and_then(|seq| self.asked.get(&seq)).cloned() else {
            return;
        };
        if let Some(decided) = self.verdict(&at) {
            decided.verdict = text(event, "verdict");
            decided.pending = false;
            decided.reason = EvidenceText::of(&text(event, "reason"));
            decided.decided_by = text(event, "decidedBy");
        }
        if self.open.get(&event.gate_id).is_some_and(|(o, _)| *o == at) {
            self.open.remove(&event.gate_id);
        }
    }

    fn gate_verdict(&mut self, event: &RunEvent) {
        if let Some(pick) = event.data.get("pick") {
            let gate_title = self.gate_title(&event.gate_id);
            let number = pick.get("attempt").and_then(Value::as_i64).unwrap_or(0);
            if let Some(attempt) = self.find(&str_of(pick.get("nodeId")), number)
                && attempt.output_seq != 0
            {
                attempt.picked = Some(AttemptPick {
                    gate_id: event.gate_id.clone(),
                    gate_title,
                    seq: event.seq,
                    reason: EvidenceText::of(&text(event, "reason")),
                    decided_by: str_of(pick.get("decidedBy")),
                    candidates: (pick.get("candidates").and_then(Value::as_array))
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_i64)
                        .collect(),
                    trigger: str_of(pick.get("trigger")),
                });
            }
            return;
        }
        let waiting = self.open.get(&event.gate_id).map(|(at, _)| at.clone());
        let at = match event.data.get("requestedSeq").and_then(Value::as_u64) {
            Some(seq) if seq > 0 => self.asked.get(&seq).cloned(),
            _ => waiting.clone(),
        };
        let Some(at) = at else {
            return;
        };
        if waiting.as_ref() == Some(&at) {
            self.open.remove(&event.gate_id);
        }
        let Some(decided) = self.verdict(&at) else {
            return;
        };
        decided.seq = event.seq;
        decided.verdict = text(event, "verdict");
        decided.pending = false;
        decided.superseded = false;
        decided.reason = EvidenceText::of(&text(event, "reason"));
        decided.evidence = (event.data.get("evidence").and_then(Value::as_array))
            .into_iter()
            .flatten()
            .take(ITEMS_MAX)
            .filter_map(|item| match item {
                Value::String(s) => Some(EvidenceItem {
                    kind: String::new(),
                    text: EvidenceText::of(s),
                }),
                Value::Object(o) => Some(EvidenceItem {
                    kind: str_of(o.get("kind")),
                    text: EvidenceText::of(&str_of(o.get("text"))),
                }),
                _ => None,
            })
            .collect();
        let by = text(event, "decidedBy");
        if !by.is_empty() {
            decided.decided_by = by;
        }
    }

    /// The step's limit is spent: its working attempt stopped.
    fn pick_started(&mut self, event: &RunEvent) {
        self.picking.insert(event.node_id.clone(), true);
        let reason = (event.data.get("limit"))
            .and_then(|l| serde_json::from_value::<crate::LimitUse>(l.clone()).ok())
            .map(|l| crate::limits::reason(self.mission, &l))
            .unwrap_or_else(|| "picking the best attempt".into());
        if let Some(attempt) = self.current(&event.node_id)
            && attempt.status == RUNNING
        {
            attempt.status = STOPPED.into();
            attempt.ended_at.clone_from(&event.timestamp);
            attempt.stop_reason = Some(EvidenceText::of(&reason));
        }
    }

    /// A block, failure or cancel stops the running attempts of the steps it
    /// names, with its reason.
    fn stopped(&mut self, event: &RunEvent) {
        let mut nodes = vec![event.node_id.clone(), event.gate_id.clone()];
        for key in ["blockedNodeId", "blockedGateId", "nodeId"] {
            nodes.push(text(event, key));
        }
        if let Some(limit) = event.data.get("limit") {
            nodes.push(str_of(limit.get("nodeId")));
        }
        for dispatch in (event.data.get("openDispatches").and_then(Value::as_array))
            .into_iter()
            .flatten()
        {
            nodes.push(str_of(dispatch.get("nodeId")));
        }
        let mut reason = text(event, "reason");
        if event.kind == "run_failed" && !text(event, "detail").is_empty() {
            reason = text(event, "detail");
        }
        for node in nodes.iter().filter(|n| !n.is_empty()) {
            if let Some(attempt) = self.current(node)
                && attempt.status == RUNNING
            {
                attempt.status = STOPPED.into();
                attempt.ended_at.clone_from(&event.timestamp);
                attempt.stop_reason = Some(EvidenceText::of(&reason));
            }
        }
    }

    /// A resume carries on the attempts a block stopped before they output,
    /// except where a pick settled the step; a redispatch runs them again.
    fn resumed(&mut self, event: &RunEvent) {
        if text(event, "resumeMode") == "redispatch" {
            return;
        }
        for step in &mut self.steps {
            if self.picking.contains_key(&step.node_id) {
                continue;
            }
            if let Some(attempt) = step.attempts.last_mut()
                && attempt.status == STOPPED
                && attempt.output_seq == 0
            {
                attempt.status = RUNNING.into();
                attempt.ended_at.clear();
                attempt.stop_reason = None;
            }
        }
    }
}

/// Every attempt of every step of a run, or of the one step `only` names.
pub fn project_attempts(
    run_id: &str,
    mission: &Mission,
    events: &[RunEvent],
    only: Option<&str>,
) -> std::io::Result<RunAttempts> {
    let status = RunProjection::from_events(events, run_id)?;
    let mut walk = Walk {
        mission,
        steps: Vec::new(),
        index: HashMap::new(),
        open: HashMap::new(),
        asked: HashMap::new(),
        picking: HashMap::new(),
    };
    for event in events {
        match event.kind.as_str() {
            "node_started" => walk.started(event),
            "node_output" => walk.output(event),
            "gate_evaluating" => walk.evaluating(event, status.r#final),
            "human_input_requested" => {
                if let Some((at, asked)) = walk.open.get_mut(&event.gate_id) {
                    *asked = true;
                    walk.asked.insert(event.seq, at.clone());
                }
            }
            "human_verdict_recorded" => walk.human_answer(event),
            "pick_started" => walk.pick_started(event),
            "gate_verdict" => walk.gate_verdict(event),
            "run_blocked" | "run_failed" | "run_canceled" => walk.stopped(event),
            "run_resumed" => walk.resumed(event),
            _ => {}
        }
    }
    if status.r#final {
        let end = format!("run {}", status.status);
        for attempt in walk.steps.iter_mut().flat_map(|s| &mut s.attempts) {
            if attempt.status == RUNNING {
                attempt.status = STOPPED.into();
                attempt.ended_at.clone_from(&status.updated_at);
                attempt.stop_reason = Some(EvidenceText::of(&end));
            }
        }
    }
    let mut steps = walk.steps;
    if let Some(only) = only {
        steps.retain(|s| s.node_id == only);
    }
    spend_budget(&mut steps);
    Ok(RunAttempts {
        run_id: run_id.to_owned(),
        steps,
    })
}

/// Spends the answer's budget newest attempt first, so a long run keeps its
/// latest attempts whole and its oldest give way.
fn spend_budget(steps: &mut [StepAttempts]) {
    let mut remaining = BUDGET;
    let mut newest_first: Vec<&mut StepAttempt> =
        steps.iter_mut().flat_map(|s| &mut s.attempts).collect();
    newest_first.sort_by_key(|a| std::cmp::Reverse(a.started_seq));
    let mut spend = |t: &mut EvidenceText| {
        t.cap(remaining);
        remaining -= t.text.len();
    };
    for attempt in newest_first {
        if let Some(r) = &mut attempt.stop_reason {
            spend(r);
        }
        for o in &mut attempt.outputs {
            spend(&mut o.text);
        }
        for v in &mut attempt.verdicts {
            spend(&mut v.reason);
            for e in &mut v.evidence {
                spend(&mut e.text);
            }
        }
        if let Some(p) = &mut attempt.picked {
            spend(&mut p.reason);
        }
    }
}

impl crate::Coordinator {
    /// Every attempt of every step of run `id`, or of step `only`.
    pub fn attempts(&self, id: &str, only: Option<&str>) -> std::io::Result<crate::RunAttempts> {
        let events = self.ledger(id)?.read()?;
        let mission = self.frozen_mission(id)?;
        if let Some(step) = only
            && mission.formation(step).is_none()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("step {step:?} not found"),
            ));
        }
        crate::run_attempts::project_attempts(id, &mission, &events, only)
    }
}
