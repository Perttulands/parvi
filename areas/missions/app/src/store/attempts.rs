//! What the attempts window says about a step's attempts (Archon's
//! `attemptsModel.ts`): each attempt's outcome in the gate's own words, the
//! attempt the loop settled on, the one the window opens on, and which two
//! stand side by side.

use parvi_missions_contract::{AttemptVerdict, StepAttempt, StepAttempts};
use std::collections::HashSet;

/// Something read over HTTP for a window.
#[derive(Clone, Debug)]
pub enum Fetched<T> {
    Loading,
    Ready(T),
    Failed(String),
}

/// The attempts window: one step of one run.
#[derive(Clone, Debug)]
pub struct AttemptsWindow {
    pub node: String,
    pub run: String,
    pub step: String,
    pub title: String,
    pub data: Fetched<StepAttempts>,
    /// The run's event count the attempts were read at; a run that moved on
    /// is read again.
    pub read_at: usize,
    /// The attempt the keys are on.
    pub focus: Option<i64>,
    /// Two attempts side by side, earlier on the left, and which side has
    /// the keys.
    pub compare: Option<(i64, i64)>,
    pub side: usize,
}

impl AttemptsWindow {
    pub fn new(node: &str, run: &str, step: &str, title: &str, read_at: usize) -> Self {
        Self {
            node: node.into(),
            run: run.into(),
            step: step.into(),
            title: title.into(),
            data: Fetched::Loading,
            read_at,
            focus: None,
            compare: None,
            side: 1,
        }
    }

    pub fn attempts(&self) -> &[StepAttempt] {
        match &self.data {
            Fetched::Ready(s) => &s.attempts,
            _ => &[],
        }
    }

    pub fn numbers(&self) -> Vec<i64> {
        self.attempts().iter().map(|a| a.attempt).collect()
    }

    /// Takes a fresh read: opens on the judge's pick, the settled attempt or
    /// the latest, then follows new attempts unless the operator stepped back.
    pub fn read(&mut self, step: StepAttempts, recommended: Option<i64>) {
        let before = self.numbers().last().copied();
        self.data = Fetched::Ready(step);
        let numbers = self.numbers();
        let latest = numbers.last().copied();
        self.focus = match self.focus {
            Some(f) if numbers.contains(&f) => {
                if Some(f) == before && latest != Some(f) {
                    latest
                } else {
                    Some(f)
                }
            }
            _ => opening(self.attempts(), recommended),
        };
        if let Some((a, b)) = self.compare
            && !(numbers.contains(&a) && numbers.contains(&b))
        {
            self.compare = None;
        }
    }

    /// The attempt the keys are on: the focused one, or a side's.
    pub fn chosen(&self) -> Option<i64> {
        match self.compare {
            Some((a, b)) => Some(if self.side == 0 { a } else { b }),
            None => self.focus,
        }
    }

    /// Side by side on, around the focused attempt; or off, keeping the
    /// side with the keys as the focus.
    pub fn toggle_compare(&mut self) {
        let numbers = self.numbers();
        if let Some(pair) = self.compare.take() {
            self.focus = Some(if self.side == 0 { pair.0 } else { pair.1 });
            return;
        }
        let Some(focus) = self.focus else { return };
        if numbers.len() < 2 {
            return;
        }
        let pair = compare_pair(&numbers, focus);
        self.side = if pair.0 == focus { 0 } else { 1 };
        self.compare = Some(pair);
    }

    /// Puts `attempt` where the keys are: the focus, or the keys' side.
    pub fn choose(&mut self, attempt: i64) {
        match &mut self.compare {
            Some(pair) if self.side == 0 => pair.0 = attempt,
            Some(pair) => pair.1 = attempt,
            None => self.focus = Some(attempt),
        }
    }

    /// The arrow keys: one attempt along, passing over the other side's.
    pub fn step(&mut self, delta: i64) {
        let numbers = self.numbers();
        let Some(at) = self.chosen().or(numbers.last().copied()) else {
            return;
        };
        let other = self
            .compare
            .map(|(a, b)| if self.side == 0 { b } else { a });
        let next = step_past(&numbers, at, delta, other);
        self.choose(next);
    }
}

/// How an attempt ended, as its gates judged it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Picked,
    Pass,
    Fail,
    Waiting,
    Judging,
    Running,
    Stopped,
    Done,
}

pub fn outcome_word(o: Outcome) -> &'static str {
    match o {
        Outcome::Picked => "picked",
        Outcome::Pass => "✓ pass",
        Outcome::Fail => "✕ fail",
        Outcome::Waiting => "waiting for you",
        Outcome::Judging => "judging",
        Outcome::Running => "running",
        Outcome::Stopped => "stopped",
        Outcome::Done => "done",
    }
}

/// `waiting` names the gates that wait for the operator.
pub fn outcome(a: &StepAttempt, waiting: &HashSet<String>) -> Outcome {
    if a.picked.is_some() {
        return Outcome::Picked;
    }
    if a.status == "running" {
        return Outcome::Running;
    }
    let pending: Vec<_> = a.verdicts.iter().filter(|v| v.pending).collect();
    if !pending.is_empty() {
        return if pending.iter().any(|v| waiting.contains(&v.gate_id)) {
            Outcome::Waiting
        } else {
            Outcome::Judging
        };
    }
    if a.verdicts.iter().any(|v| v.verdict == "fail") {
        return Outcome::Fail;
    }
    if a.verdicts.iter().any(|v| v.verdict == "pass") {
        return Outcome::Pass;
    }
    if a.status == "stopped" {
        Outcome::Stopped
    } else {
        Outcome::Done
    }
}

/// The attempt the loop settled on: the one picked, else the latest that
/// passed.
pub fn settled(attempts: &[StepAttempt]) -> Option<i64> {
    if let Some(p) = attempts.iter().find(|a| a.picked.is_some()) {
        return Some(p.attempt);
    }
    let none = HashSet::new();
    (attempts.iter().rev())
        .find(|a| outcome(a, &none) == Outcome::Pass)
        .map(|a| a.attempt)
}

/// The judge's pick, then the settled attempt, then the latest, or the
/// latest with output when the latest stopped before its own.
pub fn opening(attempts: &[StepAttempt], recommended: Option<i64>) -> Option<i64> {
    if let Some(r) = recommended.filter(|r| attempts.iter().any(|a| a.attempt == *r)) {
        return Some(r);
    }
    let latest = attempts.last()?;
    settled(attempts).or_else(|| {
        if latest.status == "stopped" && latest.outputs.is_empty() {
            (attempts.iter().rev())
                .find(|a| !a.outputs.is_empty())
                .map(|a| a.attempt)
                .or(Some(latest.attempt))
        } else {
            Some(latest.attempt)
        }
    })
}

/// The focused attempt and the one before it, or after it when it is first.
pub fn compare_pair(numbers: &[i64], focus: i64) -> (i64, i64) {
    let at = numbers.iter().position(|n| *n == focus).unwrap_or(0);
    if numbers.len() < 2 {
        let only = numbers.first().copied().unwrap_or(focus);
        return (only, only);
    }
    if at > 0 {
        (numbers[at - 1], numbers[at])
    } else {
        (numbers[0], numbers[1])
    }
}

/// One attempt along, held at the ends.
fn step_to(numbers: &[i64], current: i64, delta: i64) -> i64 {
    if numbers.is_empty() {
        return current;
    }
    let last = numbers.len() as i64 - 1;
    let from = match numbers.iter().position(|n| *n == current) {
        Some(i) => i as i64,
        None if delta > 0 => -1,
        None => last + 1,
    };
    numbers[(from + delta).clamp(0, last) as usize]
}

/// A step that passes over the attempt on the other side; held at the ends.
fn step_past(numbers: &[i64], current: i64, delta: i64, other: Option<i64>) -> i64 {
    let next = step_to(numbers, current, delta);
    if Some(next) != other {
        return next;
    }
    let beyond = step_to(numbers, next, delta);
    if beyond == next { current } else { beyond }
}

/// A verdict as one line: "Review · ✕ fail · you".
pub fn verdict_headline(v: &AttemptVerdict, waiting: &HashSet<String>) -> String {
    let gate = if v.gate_title.is_empty() {
        &v.gate_id
    } else {
        &v.gate_title
    };
    if v.pending {
        let word = if waiting.contains(&v.gate_id) {
            "waiting for you"
        } else {
            "judging"
        };
        return format!("{gate} · {word}");
    }
    if v.superseded {
        return format!("{gate} · evaluated again before your answer");
    }
    let word = match v.verdict.as_str() {
        "pass" => outcome_word(Outcome::Pass),
        "fail" => outcome_word(Outcome::Fail),
        other => other,
    };
    let decider = if v.decided_by.starts_with("human:") {
        "you"
    } else {
        ""
    };
    [gate.as_str(), word, decider]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Who picked: you, the only candidate (Parvi took it), or the judge by
/// its step's title.
pub fn picker_words(decided_by: &str, title_of: impl Fn(&str) -> String) -> String {
    match decided_by {
        "" => String::new(),
        "parvi" => "the only candidate".into(),
        d if d.starts_with("human:") => "you".into(),
        d => match title_of(d) {
            t if t.is_empty() || t == d => "the judge".into(),
            t => t,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_missions_contract::{AttemptOutput, AttemptPick};

    fn output() -> AttemptOutput {
        serde_json::from_value(
            serde_json::json!({"portId":"out", "kind":"text", "text":{"text":"", "bytes":0}}),
        )
        .unwrap()
    }
    fn verdict(gate: &str, verdict: &str, pending: bool) -> AttemptVerdict {
        serde_json::from_value(serde_json::json!({"gateId":gate,"gateTitle":"Review","evaluationSeq":1,"verdict":verdict,"decidedBy":"human:operator","pending":pending,"reason":{"text":"","bytes":0},"evidence":[]})).unwrap()
    }
    fn make_pick() -> AttemptPick {
        serde_json::from_value(serde_json::json!({"gateId":"gate","seq":1,"reason":{"text":"","bytes":0},"decidedBy":"human:operator","candidates":[],"trigger":""})).unwrap()
    }
    fn attempt(n: i64, status: &str, v: &str) -> StepAttempt {
        serde_json::from_value(serde_json::json!({"attempt":n,"status":status,"startedSeq":1,"startedAt":"","outputs":[output()],"verdicts": if v.is_empty() { vec![] } else { vec![verdict("gate",v,false)] }})).unwrap()
    }
    fn step(attempts: Vec<StepAttempt>) -> StepAttempts {
        StepAttempts {
            node_id: "s".into(),
            title: "Work".into(),
            judge_of: vec![],
            outputs: vec![],
            attempts,
        }
    }

    #[test]
    fn an_attempt_reads_as_its_gates_judged_it() {
        let none = HashSet::new();
        assert_eq!(outcome(&attempt(1, "done", "fail"), &none), Outcome::Fail);
        assert_eq!(outcome(&attempt(1, "done", ""), &none), Outcome::Done);
        assert_eq!(outcome(&attempt(1, "running", ""), &none), Outcome::Running);
        let mut asked = attempt(1, "done", "");
        asked.verdicts.push(verdict("gate", "", true));
        assert_eq!(outcome(&asked, &none), Outcome::Judging);
        let waiting = HashSet::from(["gate".to_string()]);
        assert_eq!(outcome(&asked, &waiting), Outcome::Waiting);
        let mut picked = attempt(2, "done", "fail");
        picked.picked = Some(make_pick());
        assert_eq!(outcome(&picked, &none), Outcome::Picked);
        assert_eq!(
            verdict_headline(&attempt(1, "done", "fail").verdicts[0], &none),
            "Review · ✕ fail · you"
        );
    }

    #[test]
    fn the_window_opens_on_the_judge_s_pick_then_the_settled_then_the_latest() {
        let three = vec![
            attempt(1, "done", "fail"),
            attempt(2, "done", "pass"),
            attempt(3, "done", "fail"),
        ];
        assert_eq!(opening(&three, Some(1)), Some(1));
        assert_eq!(opening(&three, Some(9)), Some(2));
        let mut stopped = vec![attempt(1, "done", "fail"), attempt(2, "stopped", "")];
        stopped[1].outputs.clear();
        assert_eq!(opening(&stopped, None), Some(1));
        let mut picked = three.clone();
        picked[2].picked = Some(make_pick());
        assert_eq!(settled(&picked), Some(3));
    }

    #[test]
    fn side_by_side_puts_the_focus_beside_the_one_before_and_the_keys_pass_over_the_other() {
        assert_eq!(compare_pair(&[1, 2, 3], 3), (2, 3));
        assert_eq!(compare_pair(&[1, 2, 3], 1), (1, 2));
        let mut w = AttemptsWindow::new("n", "r", "s", "Work", 0);
        w.read(
            step(vec![
                attempt(1, "done", "fail"),
                attempt(2, "done", "fail"),
                attempt(3, "done", "fail"),
            ]),
            None,
        );
        assert_eq!(w.focus, Some(3));
        w.toggle_compare();
        assert_eq!((w.compare, w.side), (Some((2, 3)), 1));
        // The right side steps left over the left side's attempt 2.
        w.step(-1);
        assert_eq!(w.compare, Some((2, 1)));
        w.step(1);
        assert_eq!(w.compare, Some((2, 3)));
        w.toggle_compare();
        assert_eq!((w.compare, w.focus), (None, Some(3)));
    }

    #[test]
    fn a_fresh_read_follows_a_new_attempt_unless_the_operator_stepped_back() {
        let mut w = AttemptsWindow::new("n", "r", "s", "Work", 0);
        let read = |n: i64| step((1..=n).map(|i| attempt(i, "done", "fail")).collect());
        w.read(read(2), None);
        w.read(read(3), None);
        assert_eq!(w.focus, Some(3));
        w.choose(1);
        w.read(read(4), None);
        assert_eq!(w.focus, Some(1));
    }
}
