//! Where a run is, read from its ledger events: each step's state, the
//! places it is now (Archon's `formationsRunState.ts`, ported as it is), and
//! the run bar's words for them.

use parvi_missions_contract::{PublicEvent as RunEvent, RunProjection as Run};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeState {
    Running,
    Done,
    Blocked,
    Waiting,
    Failed,
    Canceled,
}

/// Each node's state, the seq it last changed at, its latest attempt, and
/// which nodes are gates.
#[derive(Default, Debug)]
pub struct Projection {
    pub states: HashMap<String, NodeState>,
    changed_at: HashMap<String, u64>,
    pub attempts: HashMap<String, i64>,
    gates: HashSet<String>,
}

impl Projection {
    fn set(&mut self, node: &str, state: NodeState, seq: u64) {
        self.states.insert(node.to_string(), state);
        self.changed_at.insert(node.to_string(), seq);
    }
}

fn sorted(events: &[RunEvent]) -> Vec<&RunEvent> {
    let mut v: Vec<&RunEvent> = events.iter().collect();
    v.sort_by_key(|e| e.seq);
    v
}

/// What a running step's seat last reported about itself when that is not
/// plain work: an agent that ended its turn without finishing, or the one
/// nudge the coordinator sent it. Blank once the seat works or finishes.
pub fn seat_note(events: &[RunEvent], node: &str) -> Option<String> {
    let last = sorted(events)
        .into_iter()
        .rev()
        .find(|e| e.node_id == node)?;
    match last.kind.as_str() {
        "seat_state" if last.state != "working" && !last.detail.is_empty() => {
            Some(last.detail.clone())
        }
        "seat_idle_recovery" if !last.detail.is_empty() => Some(last.detail.clone()),
        _ => None,
    }
}

/// A recorded problem as it reads now: a block the run has since resumed
/// from no longer tells the operator to resume it.
pub fn problem_now(events: &[RunEvent], seq: u64, reason: &str) -> String {
    let resumed = events
        .iter()
        .any(|e| e.kind == "run_resumed" && e.seq > seq);
    match reason.split_once("; resume") {
        Some((what, _)) if resumed => format!("{what}; resumed"),
        _ => reason.to_owned(),
    }
}

/// Honest per-node state from the ledger, in the engine's vocabulary.
pub fn project(events: &[RunEvent]) -> Projection {
    let mut p = Projection::default();
    // A block holds the node only until the run resumes.
    let mut before_block: Vec<(String, Option<NodeState>)> = vec![];
    // A gate answered while blocked keeps waiting until the resume routes it.
    let mut blocked = false;
    let mut answered_blocked: Vec<String> = vec![];
    for e in sorted(events) {
        let seq = e.seq;
        match e.kind.as_str() {
            "run_resumed" => {
                for (id, prior) in before_block.drain(..) {
                    match prior {
                        Some(s) => p.set(&id, s, seq),
                        None => {
                            p.states.remove(&id);
                            p.changed_at.insert(id, seq);
                        }
                    }
                }
                for id in answered_blocked.drain(..) {
                    p.set(&id, NodeState::Running, seq);
                }
                blocked = false;
                continue;
            }
            "run_canceled" => {
                let open: Vec<String> = p
                    .states
                    .iter()
                    .filter(|(_, s)| {
                        matches!(
                            s,
                            NodeState::Running | NodeState::Waiting | NodeState::Blocked
                        )
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in open {
                    p.set(&id, NodeState::Canceled, seq);
                }
                continue;
            }
            "run_blocked" => blocked = true,
            _ => {}
        }
        if matches!(
            e.kind.as_str(),
            "run_succeeded" | "run_picked" | "run_failed"
        ) {
            for end in &e.end_ids {
                p.set(end, NodeState::Done, seq);
            }
        }
        let node = if e.node_id.is_empty() {
            e.gate_id.as_str()
        } else {
            e.node_id.as_str()
        };
        if node.is_empty() {
            // A block or failure naming no node stops whatever was in flight.
            if e.kind == "run_blocked" || e.kind == "run_failed" {
                let running: Vec<String> = p
                    .states
                    .iter()
                    .filter(|(_, s)| **s == NodeState::Running)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in running {
                    let block = e.kind == "run_blocked";
                    if block && !before_block.iter().any(|(b, _)| *b == id) {
                        before_block.push((id.clone(), Some(NodeState::Running)));
                    }
                    let to = if block {
                        NodeState::Blocked
                    } else {
                        NodeState::Failed
                    };
                    p.set(&id, to, seq);
                }
            }
            continue;
        }
        if !e.gate_id.is_empty() {
            p.gates.insert(e.gate_id.clone());
        }
        if e.attempt > 0 && (e.kind == "node_started" || e.kind == "slot_dispatch") {
            p.attempts.insert(node.to_string(), e.attempt);
        }
        match e.kind.as_str() {
            "node_started" | "slot_dispatch" | "gate_evaluating" => {
                p.set(node, NodeState::Running, seq)
            }
            "human_input_requested" | "node_waiting" => p.set(node, NodeState::Waiting, seq),
            "human_verdict_recorded" => {
                if blocked {
                    answered_blocked.push(node.to_string());
                } else {
                    p.set(node, NodeState::Running, seq);
                }
            }
            "node_output" => {
                let s = if e.status == "blocked" {
                    NodeState::Blocked
                } else {
                    NodeState::Done
                };
                p.set(node, s, seq);
            }
            "gate_verdict" => {
                let s = if e.verdict == "fail" {
                    NodeState::Failed
                } else {
                    NodeState::Done
                };
                p.set(node, s, seq);
            }
            "pick_started" => {
                p.set(node, NodeState::Done, seq);
                if !e.gate_id.is_empty() {
                    let g = e.gate_id.clone();
                    p.set(&g, NodeState::Running, seq);
                }
            }
            "error" | "run_blocked" => {
                if !before_block.iter().any(|(b, _)| b == node) {
                    before_block.push((node.to_string(), p.states.get(node).copied()));
                }
                p.set(node, NodeState::Blocked, seq);
            }
            "run_failed" => p.set(node, NodeState::Failed, seq),
            _ => {}
        }
    }
    p
}

/// The gates waiting for a verdict, oldest first: a request waits until a
/// verdict on its gate, a newer evaluation of it, or the run's end.
pub fn open_human_gates(events: &[RunEvent]) -> Vec<String> {
    let mut open: Vec<String> = vec![];
    for e in sorted(events) {
        let gate = if e.gate_id.is_empty() {
            &e.node_id
        } else {
            &e.gate_id
        };
        match e.kind.as_str() {
            "human_input_requested" => {
                open.retain(|g| g != gate);
                open.push(gate.clone());
            }
            "human_verdict_recorded" | "gate_evaluating" => open.retain(|g| g != gate),
            "run_succeeded" | "run_picked" | "run_failed" | "run_canceled" => open.clear(),
            _ => {}
        }
    }
    open
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointKind {
    Waiting,
    Running,
    Blocked,
    Failed,
    Canceled,
}

/// A place a run is now: a node it waits at, runs, or stopped on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Point {
    pub kind: PointKind,
    /// Blank when the ledger names no node.
    pub node: String,
    pub gate: bool,
    pub attempt: i64,
    /// The block's recorded detail, for a block.
    pub reason: String,
    /// A running gate whose operator verdict the run is about to route.
    pub answered: bool,
    pub picking: bool,
}

impl Point {
    fn new(kind: PointKind, node: &str, gate: bool) -> Self {
        Self {
            kind,
            node: node.to_string(),
            gate,
            attempt: 0,
            reason: String::new(),
            answered: false,
            picking: false,
        }
    }
}

/// Every place a run is now, the one most needing the operator first:
/// each gate waiting for them, then each step and gate running. A blocked
/// run names its block first; a failed or canceled run where it stopped.
pub fn points(run: &Run) -> Vec<Point> {
    let p = project(&run.events);
    let events = sorted(&run.events);
    let latest = |state: NodeState| -> String {
        p.states
            .iter()
            .filter(|(_, s)| **s == state)
            .max_by_key(|(id, _)| (p.changed_at.get(*id).copied().unwrap_or(0), (*id).clone()))
            .map(|(id, _)| id.clone())
            .unwrap_or_default()
    };
    let waiting_ids: Vec<String> = if run.waiting_gates.is_empty() {
        open_human_gates(&run.events)
    } else {
        run.waiting_gates
            .iter()
            .map(|g| g.gate_id.clone())
            .collect()
    };
    let waiting: Vec<Point> = waiting_ids
        .iter()
        .map(|id| Point::new(PointKind::Waiting, id, true))
        .collect();
    let mut answered = HashSet::new();
    let mut picking = HashSet::new();
    for e in &events {
        let gate = if e.gate_id.is_empty() {
            e.node_id.clone()
        } else {
            e.gate_id.clone()
        };
        match e.kind.as_str() {
            "human_verdict_recorded" => {
                answered.insert(gate);
            }
            "pick_started" if !e.gate_id.is_empty() => {
                picking.insert(e.gate_id.clone());
            }
            "gate_verdict" | "gate_evaluating" => {
                answered.remove(&gate);
                picking.remove(&gate);
            }
            _ => {}
        }
    }
    let mut running: Vec<(&String, u64)> = p
        .states
        .iter()
        .filter(|(id, s)| **s == NodeState::Running && !waiting_ids.contains(id))
        .map(|(id, _)| (id, p.changed_at.get(id).copied().unwrap_or(0)))
        .collect();
    running.sort_by_key(|(id, at)| (*at, (*id).clone()));
    let running: Vec<Point> = running
        .into_iter()
        .map(|(id, _)| {
            let mut pt = Point::new(PointKind::Running, id, p.gates.contains(id));
            pt.attempt = p.attempts.get(id).copied().unwrap_or(0);
            pt.answered = answered.contains(id);
            pt.picking = !pt.answered && picking.contains(id);
            pt
        })
        .collect();
    match run.status.as_str() {
        "waiting_human" | "running" => waiting.into_iter().chain(running).collect(),
        "blocked" => {
            let block = events.iter().rev().find(|e| e.kind == "run_blocked");
            let node = block
                .map(|b| {
                    if b.gate_id.is_empty() {
                        b.node_id.clone()
                    } else {
                        b.gate_id.clone()
                    }
                })
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| latest(NodeState::Blocked));
            let mut pt = Point::new(PointKind::Blocked, &node, p.gates.contains(&node));
            pt.reason = block
                .map(|b| {
                    if b.detail.is_empty() {
                        b.code.replace('_', " ")
                    } else {
                        parvi_missions_contract::plain_reason(&b.detail)
                    }
                })
                .unwrap_or_else(|| run.block_code.replace('_', " "));
            std::iter::once(pt)
                .chain(waiting.into_iter().filter(|w| w.node != node))
                .collect()
        }
        "failed" => {
            let node = latest(NodeState::Failed);
            vec![Point::new(
                PointKind::Failed,
                &node,
                p.gates.contains(&node),
            )]
        }
        "canceled" => {
            let before: Vec<RunEvent> = run
                .events
                .iter()
                .filter(|e| e.kind != "run_canceled")
                .cloned()
                .collect();
            let node = open_human_gates(&before)
                .into_iter()
                .next()
                .unwrap_or_else(|| latest(NodeState::Canceled));
            vec![Point::new(
                PointKind::Canceled,
                &node,
                p.gates.contains(&node),
            )]
        }
        _ => vec![],
    }
}

/// The run bar's words for a point; `title` names its node.
pub fn phrase(point: &Point, title: &str) -> String {
    let at = if title.is_empty() { &point.node } else { title };
    match point.kind {
        PointKind::Waiting => format!("waiting for you at {at}"),
        PointKind::Running if point.answered => format!("routing your answer at {at}"),
        PointKind::Running if point.picking => format!("picking the best attempt at {at}"),
        PointKind::Running if point.gate => format!("evaluating {at}"),
        PointKind::Running if point.attempt > 1 => {
            format!("running {at} (attempt {})", point.attempt)
        }
        PointKind::Running => format!("running {at}"),
        PointKind::Blocked => {
            let mut s = if at.is_empty() {
                "blocked".to_string()
            } else {
                format!("blocked at {at}")
            };
            if !point.reason.is_empty() {
                s.push_str(&format!(": {}", point.reason));
            }
            s
        }
        PointKind::Failed if at.is_empty() => String::new(),
        PointKind::Failed => format!("failed at {at}"),
        PointKind::Canceled if at.is_empty() => String::new(),
        PointKind::Canceled => format!("canceled at {at}"),
    }
}

/// What earlier runs produced that could fill an input of `kind`: the
/// latest first, each value once, at most `most`, as (label, value).
pub fn earlier_outputs<'a>(
    runs: impl IntoIterator<Item = &'a Run>,
    kind: &str,
    most: usize,
) -> Vec<(String, String)> {
    let mut runs: Vec<&Run> = runs.into_iter().filter(|run| run.r#final).collect();
    runs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let mut found: Vec<(String, String)> = Vec::new();
    for run in runs {
        for output in run.outputs.iter().filter(|o| o.kind == kind) {
            if found.len() < most && !found.iter().any(|(_, value)| value == &output.value) {
                let label = format!("{} · {}", output.name, run.mission_slug);
                found.push((label, output.value.clone()));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_run(status: &str) -> Run {
        serde_json::from_value(serde_json::json!({"runId":"r", "status":status, "final":false, "resumeAllowed":false, "epoch":1, "missionSlug":"m", "missionId":"m", "missionRev":1, "inputCardId":"i", "work":"", "cwd":"", "repository":"", "base":"", "contextPaths":[], "inputs":[], "outputs":[], "eventCount":0, "startedBy":"", "startedAt":"", "updatedAt":"", "endedBy":"", "waitingGates":[], "events":[]})).unwrap()
    }

    fn ev(seq: u64, kind: &str, node: &str) -> RunEvent {
        serde_json::from_value(serde_json::json!({"seq":seq, "type":kind, "nodeId":node})).unwrap()
    }

    fn gate(seq: u64, kind: &str, g: &str) -> RunEvent {
        RunEvent {
            gate_id: g.into(),
            ..ev(seq, kind, g)
        }
    }

    #[test]
    fn a_waiting_gate_comes_before_running_steps() {
        let mut run = test_run("waiting_human");
        run.events = vec![
            ev(1, "run_started", ""),
            ev(2, "node_started", "a"),
            ev(3, "node_output", "a"),
            ev(4, "node_started", "b"),
            gate(5, "gate_evaluating", "g"),
            gate(6, "human_input_requested", "g"),
        ];
        let pts = points(&run);
        assert_eq!(pts[0].kind, PointKind::Waiting);
        assert_eq!(pts[0].node, "g");
        assert_eq!(pts[1].node, "b");
        let p = project(&run.events);
        assert_eq!(p.states["a"], NodeState::Done);
        assert_eq!(p.states["g"], NodeState::Waiting);
        assert_eq!(
            phrase(&pts[0], "Operator verdict"),
            "waiting for you at Operator verdict"
        );
    }

    #[test]
    fn a_resumed_block_no_longer_asks_for_a_resume() {
        let reason = "coordinator shut down; resume to continue";
        let blocked = vec![ev(1, "run_started", ""), ev(4, "run_blocked", "")];
        assert_eq!(problem_now(&blocked, 4, reason), reason);
        let resumed = [blocked, vec![ev(5, "run_resumed", "")]].concat();
        assert_eq!(
            problem_now(&resumed, 4, reason),
            "coordinator shut down; resumed"
        );
    }

    #[test]
    fn a_block_holds_its_node_until_resume() {
        let mut blocked = ev(4, "run_blocked", "b");
        blocked.code = "invalid_judge_result".into();
        let mut events = vec![
            ev(1, "run_started", ""),
            ev(2, "node_started", "a"),
            ev(3, "node_started", "b"),
            blocked,
        ];
        let mut run = test_run("blocked");
        run.events = events.clone();
        let pts = points(&run);
        assert_eq!(pts[0].kind, PointKind::Blocked);
        assert_eq!(phrase(&pts[0], "B"), "blocked at B: invalid judge result");
        events.push(ev(5, "run_resumed", ""));
        assert_eq!(project(&events).states["b"], NodeState::Running);
    }

    #[test]
    fn an_answer_closes_the_request_and_a_cancel_stops_open_work() {
        let events = vec![
            gate(1, "human_input_requested", "g"),
            gate(2, "human_verdict_recorded", "g"),
        ];
        assert!(open_human_gates(&events).is_empty());
        let mut run = test_run("canceled");
        run.events = vec![
            ev(1, "node_started", "a"),
            gate(2, "human_input_requested", "g"),
            ev(3, "run_canceled", ""),
        ];
        assert_eq!(points(&run)[0].node, "g");
        assert_eq!(project(&run.events).states["a"], NodeState::Canceled);
    }

    #[test]
    fn a_seat_that_stopped_short_says_so_until_it_works_again() {
        let stopped = RunEvent {
            state: "turn_ended_without_sentinel".into(),
            detail: "the agent ended a turn without the completion sentinel".into(),
            ..ev(2, "seat_state", "work")
        };
        let mut events = vec![ev(1, "adapter_send", "work"), stopped];
        assert_eq!(
            seat_note(&events, "work").as_deref(),
            Some("the agent ended a turn without the completion sentinel")
        );
        events.push(RunEvent {
            state: "working".into(),
            ..ev(3, "seat_state", "work")
        });
        assert_eq!(seat_note(&events, "work"), None);
    }

    #[test]
    fn earlier_outputs_offer_finished_runs_latest_first_by_kind() {
        let output = |name: &str, kind: &str, value: &str| serde_json::json!({"name":name,"kind":kind,"value":value,"nodeId":"s","portId":"out","attempt":1});
        let mut old = test_run("succeeded");
        old.r#final = true;
        old.updated_at = "2026-11-10T10:00:00Z".into();
        old.outputs = serde_json::from_value(serde_json::json!([
            output("plan", "file", "/w/plan.txt"),
            output("note", "text", "hello")
        ]))
        .unwrap();
        let mut new = test_run("succeeded");
        new.r#final = true;
        new.mission_slug = "review".into();
        new.updated_at = "2026-11-11T10:00:00Z".into();
        new.outputs = serde_json::from_value(serde_json::json!([
            output("review", "file", "/w/review.txt"),
            output("plan", "file", "/w/plan.txt")
        ]))
        .unwrap();
        let mut going = test_run("running");
        going.outputs = new.outputs.clone();
        going.outputs[0].value = "/w/partial.txt".into();
        let files = earlier_outputs([&old, &new, &going], "file", 5);
        assert_eq!(
            files,
            [
                ("review · review".to_owned(), "/w/review.txt".to_owned()),
                ("plan · review".to_owned(), "/w/plan.txt".to_owned())
            ]
        );
        assert_eq!(earlier_outputs([&old, &new], "text", 5).len(), 1);
        assert_eq!(earlier_outputs([&old, &new], "file", 1).len(), 1);
    }
}
