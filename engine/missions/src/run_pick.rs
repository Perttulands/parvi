//! Best of N (Archon `run_pick.go`). A step whose own Limit card is spent
//! has attempts its gate already judged. A card whose whenSpent is best, or a
//! resume with `--pick-best`, picks the best of them instead of stopping: the
//! gate's judge chain compares them side by side, or the operator picks, and
//! the picked attempt goes on through the gate's pass route with its output,
//! as if the gate had passed it.
//!
//! The ledger records a pick in two events. `pick_started` names the step,
//! its selecting gate, the spent limit and the candidate attempts; it settles
//! the step's pending send-back, as the step's output would. The pick ends in
//! a `gate_verdict` pass whose `pick` names the attempt, the candidates, why
//! and who picked. Between the two a judge chain runs, anchored at
//! `pick_started`, or a `human_input_requested` with a `pick` waits for the
//! operator. Replay finishes a pick the ledger started and did not end.
use crate::coordinator::Control;
use crate::limits::LimitUse;
use crate::run_limit::LimitStop;
use crate::run_worker::event;
use crate::{Coordinator, Gate, Mission, RoutedInput, RunEvent, RunProjection, RunStart};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io};

pub(crate) const PICK_STARTED: &str = "pick_started";
/// The block a judge's malformed pick records; a plain resume asks the final
/// judge again.
pub(crate) const INVALID_JUDGE_PICK: &str = "invalid_judge_pick";
/// Who decides a pick with only one candidate (Archon says `archon`).
const DECIDED_BY_PARVI: &str = "parvi";

/// An attempt a pick may take: the step's output was accepted (`outputSeq`)
/// and its selecting gate judged it (`verdictSeq`). An attempt a limit cut
/// off, or a crash ended, has no output and is never one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Candidate {
    pub attempt: i64,
    pub output_seq: u64,
    pub verdict_seq: u64,
}

/// What picking a step's best attempt takes: its selecting gate and the
/// attempts that gate judged.
pub(crate) struct StepPick<'a> {
    pub gate: &'a Gate,
    pub candidates: Vec<Candidate>,
}

/// A decided pick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Choice {
    pub attempt: i64,
    pub reason: String,
    pub evidence: Vec<String>,
    pub decided_by: String,
}
impl Choice {
    fn data(&self) -> Value {
        let evidence: Vec<_> = (self.evidence.iter())
            .map(|text| json!({"kind":"formation","text":text}))
            .collect();
        json!({"attempt":self.attempt,"reason":self.reason,"evidence":evidence,"decidedBy":self.decided_by})
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}
fn data(event: &RunEvent) -> &serde_json::Map<String, Value> {
    &event.data
}

/// Whether an event belongs to a pick: a pick's gate verdict, its human
/// request, or the operator's pick.
pub(crate) fn is_pick(event: &RunEvent) -> bool {
    event.data.contains_key("pick")
}
/// The `pick_started` a pick's verdict or request answers.
pub(crate) fn pick_seq(event: &RunEvent) -> u64 {
    (event.data.get("pick"))
        .and_then(|pick| pick.get("pickSeq"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// The step's attempts whose output the gate judged with a verdict of its
/// own, by attempt; a pick's verdict judges nothing. Each verdict names the
/// output it judged (its inputRef's origin). An attempt's latest verdict
/// counts.
pub(crate) fn candidates(events: &[RunEvent], step: &str, gate: &str) -> Vec<Candidate> {
    let mut outputs = BTreeMap::new();
    let mut judged = BTreeMap::new();
    for event in events {
        match event.kind.as_str() {
            "node_output" if event.node_id == step => {
                outputs.insert(event.attempt, event.seq);
            }
            "gate_verdict" if event.gate_id == gate && !is_pick(event) => {
                let port = data(event).get("routePort").and_then(Value::as_str);
                if port != Some("pass") && port != Some("fail") {
                    continue;
                }
                let origin = (data(event).get("inputRef"))
                    .and_then(|input| input.get("origin"))
                    .and_then(|origin| {
                        serde_json::from_value::<crate::PublicOrigin>(origin.clone()).ok()
                    });
                let Some(origin) = origin.filter(|origin| origin.node_id == step) else {
                    continue;
                };
                let Some(output_seq) = outputs.get(&origin.attempt) else {
                    continue;
                };
                judged.insert(
                    origin.attempt,
                    Candidate {
                        attempt: origin.attempt,
                        output_seq: *output_seq,
                        verdict_seq: event.seq,
                    },
                );
            }
            _ => {}
        }
    }
    judged.into_values().collect()
}

/// Whether the spent limit `use_`, met by `step`, can pick: it is the step's
/// own card, the step has a selecting gate, and that gate judged at least one
/// of its attempts. A mission card never picks.
pub(crate) fn pick_for<'a>(
    mission: &'a Mission,
    events: &[RunEvent],
    step: &str,
    use_: &LimitUse,
) -> Option<StepPick<'a>> {
    let card = crate::limits::card(mission, &use_.limit_id)?;
    if card.target != step || use_.node_id != step {
        return None;
    }
    let gate = crate::graph::selecting_gate(mission, step).ok()?;
    let candidates = candidates(events, step, &gate.id);
    (!candidates.is_empty()).then_some(StepPick { gate, candidates })
}

/// Whether the gate judged one of the candidates after the step's latest
/// pick. A card picks by itself only then, so a pass route that leads back
/// to the step cannot pick the same attempts forever.
pub(crate) fn judged_since_last_pick(
    events: &[RunEvent],
    step: &str,
    candidates: &[Candidate],
) -> bool {
    let last = (events.iter())
        .filter(|e| e.kind == PICK_STARTED && e.node_id == step)
        .map(|e| e.seq)
        .next_back()
        .unwrap_or(0);
    candidates.iter().any(|c| c.verdict_seq > last)
}

/// The spent limit a resume with `--pick-best` asked to pick at, while that
/// resume is the run's latest and no pick started since.
fn pending_resume_pick(events: &[RunEvent]) -> Option<LimitUse> {
    for event in events.iter().rev() {
        match event.kind.as_str() {
            PICK_STARTED => return None,
            "run_resumed" => {
                if event.data.get("pickBest").and_then(Value::as_bool) != Some(true) {
                    return None;
                }
                let from = event.data.get("resumedFromSeq").and_then(Value::as_u64)?;
                let block = events.iter().find(|e| e.seq == from)?;
                return LimitUse::recorded(block);
            }
            _ => {}
        }
    }
    None
}

/// Record `pick_started`, which settles the step's pending send-back as its
/// output would; the run's worker decides the pick.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start(
    c: &Coordinator,
    id: &str,
    control: &Control,
    mission: &Mission,
    step: &str,
    pick: &StepPick<'_>,
    use_: &LimitUse,
    trigger: &str,
) -> io::Result<()> {
    let mut data = json!({"nodeId":step,"gateId":pick.gate.id,"trigger":trigger,"limit":use_,"candidates":pick.candidates});
    if pick.gate.kinds.iter().any(|kind| kind == "formation") {
        let chain: Vec<_> = (crate::graph::judge_chain(mission, &pick.gate.id).into_iter())
            .map(|judge| judge.id.clone())
            .collect();
        data["judgeChain"] = json!(chain);
    }
    let mut started = event(PICK_STARTED, step, 0, data);
    started.gate_id = pick.gate.id.clone();
    crate::run_worker::append_live(c, id, control, started).map(|_| ())
}

/// Start the pick a resume with `--pick-best` asked for, whatever the card's
/// whenSpent; where the step can no longer pick, the run stops at the limit
/// again.
pub(crate) fn on_resume(
    c: &Coordinator,
    id: &str,
    control: &Control,
    mission: &Mission,
) -> io::Result<Option<LimitStop>> {
    let events = c.ledger(id)?.read()?;
    let Some(use_) = pending_resume_pick(&events) else {
        return Ok(None);
    };
    let node = use_.node_id.clone();
    if let Some(pick) = pick_for(mission, &events, &node, &use_) {
        start(c, id, control, mission, &node, &pick, &use_, "resume")?;
        return Ok(None);
    }
    crate::run_limit::append_limit_error(c, id, control, mission, &node, &use_)?;
    Ok(Some(LimitStop {
        node,
        use_,
        fail: false,
    }))
}

/// The picks the ledger started and neither ended nor put to the operator.
pub(crate) fn unsettled(events: &[RunEvent]) -> Vec<&RunEvent> {
    (events.iter())
        .filter(|started| started.kind == PICK_STARTED)
        .filter(|started| {
            !events.iter().any(|e| {
                (e.kind == "gate_verdict" || e.kind == "human_input_requested")
                    && pick_seq(e) == started.seq
            })
        })
        .collect()
}

/// A candidate as the ledger shows it: the verdict its gate gave it and the
/// gate input that verdict judged, which a pick carries on.
struct Detail {
    attempt: i64,
    verdict: String,
    reason: String,
    input: Value,
}

fn details(events: &[RunEvent], started: &RunEvent) -> io::Result<Vec<Detail>> {
    let candidates: Vec<Candidate> =
        serde_json::from_value(started.data.get("candidates").cloned().unwrap_or_default())
            .map_err(|e| invalid(&format!("pick {} candidates: {e}", started.seq)))?;
    let mut details = Vec::new();
    for candidate in candidates {
        let verdict = (events.iter())
            .find(|e| e.seq == candidate.verdict_seq && e.kind == "gate_verdict")
            .ok_or_else(|| {
                invalid(&format!(
                    "pick candidate attempt {} names verdict {}",
                    candidate.attempt, candidate.verdict_seq
                ))
            })?;
        details.push(Detail {
            attempt: candidate.attempt,
            verdict: text(&Value::Object(verdict.data.clone()), "verdict").into(),
            reason: text(&Value::Object(verdict.data.clone()), "reason").into(),
            input: verdict.data.get("inputRef").cloned().unwrap_or_default(),
        });
    }
    if details.is_empty() {
        return Err(invalid(&format!("pick {} has no candidates", started.seq)));
    }
    Ok(details)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

/// What a picker reads: every candidate side by side, with the verdict the
/// gate gave it and the output it judged, as text or as the path of the kept
/// file or folder.
fn pick_input(
    id: &str,
    mission: &Mission,
    gate: &Gate,
    started: &RunEvent,
    details: &[Detail],
) -> RoutedInput {
    let mut body = format!(
        "Pick the best attempt of {}. Its Limit card is spent, so it runs no more; the attempt picked goes on through {} as if it had passed.\n",
        mission.node_name(&started.node_id),
        mission.node_name(&gate.id)
    );
    if !gate.criterion.trim().is_empty() {
        body += &format!("criterion: {}\n", gate.criterion.trim());
    }
    for detail in details {
        body += &format!(
            "\nattempt {}\nverdict: {}\nreason: {}\n",
            detail.attempt, detail.verdict, detail.reason
        );
        let kind = text(&detail.input, "kind");
        let reference = text(&detail.input, "ref");
        if (kind == "file" || kind == "folder") && !reference.is_empty() {
            body += &format!("output {kind}: {reference}\n");
        }
        let output = text(&detail.input, "text");
        if !output.is_empty() {
            body += &format!("output:\n{output}\n");
        }
    }
    RoutedInput {
        from_node_id: started.node_id.clone(),
        r#ref: format!("ledger://{id}/pick/{}", started.seq),
        text: body,
        ..Default::default()
    }
}

/// Attempt numbers said plainly: "1, 2 or 3".
pub(crate) fn attempt_words(attempts: &[i64]) -> String {
    let words: Vec<_> = attempts.iter().map(i64::to_string).collect();
    match words.as_slice() {
        [] => "none".into(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// What a judge in a pick returns: the final judge picks with a fenced
/// `parvi-pick` block, the judges before it recommend in plain output.
pub(crate) fn contract(final_judge: bool, attempts: &[i64]) -> String {
    let pick = crate::contract_names::PICK;
    if !final_judge {
        return format!(
            "judge recommend contract:\nThis is a pick, not a verdict, and a later judge in the chain decides it: compare the attempts below side by side and recommend the best one. Write your recommendation as your output: the attempt you recommend (one of attempts {}), why it is the best, and what decided it. The judges after you read it beside the attempts. Emit no {pick} or {} block.\n",
            attempt_words(attempts),
            crate::contract_names::VERDICT
        );
    }
    format!(
        "judge pick contract:\nThis is a pick, not a verdict: compare the attempts below side by side and pick the best one. Before the completion sentinel, emit exactly one fenced {pick} JSON block, separate from {}, with exactly these three keys:\n```{pick}\n{{\"attempt\":{},\"reason\":\"Why this attempt is the best\",\"evidence\":[\"what decided it\"]}}\n```\nattempt must be one of attempts {}, reason a string, and evidence an array of strings (use [] when there are no references). Do not omit evidence or add keys. Your pick decides; any recommendation from an earlier judge follows the attempts.\n",
        crate::contract_names::OUTPUTS,
        attempts.first().copied().unwrap_or(1),
        attempt_words(attempts)
    )
}

/// Accept one explicitly fenced pick of a candidate attempt, never prose.
pub(crate) fn parse_pick(text: &str, attempts: &[i64]) -> Result<Choice, String> {
    let lang = crate::contract_names::PICK;
    let marker = format!("```{lang}");
    let mut body = String::new();
    let (mut blocks, mut inside, mut closed) = (0, false, false);
    for line in text.lines().map(str::trim) {
        if line.starts_with(&marker) {
            blocks += 1;
            if line != marker || inside || blocks != 1 {
                return Err(format!("expected exactly one {lang} block"));
            }
            inside = true;
        } else if inside && line == "```" {
            inside = false;
            closed = true;
        } else if inside {
            body.push_str(line);
            body.push('\n');
        }
    }
    if blocks != 1 || inside || !closed {
        return Err(format!("missing or unterminated {lang} block"));
    }
    const KEYS: &str = "requires exactly attempt, reason and evidence";
    let fields: serde_json::Map<String, Value> =
        serde_json::from_str(&body).map_err(|_| "judge pick must be a JSON object".to_owned())?;
    if let Some(extra) = fields
        .keys()
        .find(|key| !["attempt", "reason", "evidence"].contains(&key.as_str()))
    {
        return Err(format!("judge pick has extra key {extra:?}; it {KEYS}"));
    }
    let missing: Vec<_> = (["attempt", "reason", "evidence"].into_iter())
        .filter(|key| !fields.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "judge pick is missing {}; it {KEYS}",
            missing.join(" and ")
        ));
    }
    let attempt = (fields["attempt"].as_i64())
        .filter(|attempt| attempts.contains(attempt))
        .ok_or_else(|| {
            format!(
                "judge pick attempt must be one of attempts {}",
                attempt_words(attempts)
            )
        })?;
    let reason = (fields["reason"].as_str())
        .ok_or("judge pick reason must be a string")?
        .to_owned();
    let evidence = (fields["evidence"].as_array())
        .ok_or("judge pick evidence must be an array of strings")?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or("judge pick evidence must contain only strings")?;
    Ok(Choice {
        attempt,
        reason,
        evidence,
        decided_by: String::new(),
    })
}

fn gate_attempt(events: &[RunEvent], gate: &str) -> i64 {
    (events.iter())
        .filter(|e| e.kind == "gate_evaluating" && e.gate_id == gate)
        .count() as i64
}

/// End a pick: a gate_verdict pass that carries the picked attempt's gate
/// input down the gate's pass route, as the gate's pass would have. Replay
/// delivers it. `answer` is the operator's recorded pick and the request it
/// answers, if any.
fn resolve(
    c: &Coordinator,
    id: &str,
    control: &Control,
    mission: &Mission,
    started: &RunEvent,
    choice: &Choice,
    answer: Option<(&RunEvent, &RunEvent)>,
) -> io::Result<()> {
    let events = c.ledger(id)?.read()?;
    let details = details(&events, started)?;
    let picked = (details.iter())
        .find(|detail| detail.attempt == choice.attempt)
        .ok_or_else(|| {
            invalid(&format!(
                "pick {} names attempt {}, which is not a candidate",
                started.seq, choice.attempt
            ))
        })?;
    let attempts: Vec<_> = details.iter().map(|detail| detail.attempt).collect();
    let mut pick = json!({"nodeId":started.node_id,"attempt":choice.attempt,"pickSeq":started.seq,"candidates":attempts,"decidedBy":choice.decided_by,"trigger":text(&Value::Object(started.data.clone()), "trigger")});
    let evidence: Vec<_> = (choice.evidence.iter())
        .map(|text| json!({"kind":"formation","text":text}))
        .collect();
    let routes: Vec<_> = crate::graph::outgoing_from_port(mission, &started.gate_id, "pass")
        .map(|edge| edge.id.clone())
        .collect();
    let mut data = json!({"verdict":"pass","routePort":"pass","routedEdges":routes,"reason":choice.reason,"evidence":evidence,"inputRef":picked.input});
    let mut actor = String::new();
    if let Some((request, recorded)) = answer {
        data["requestedSeq"] = request.seq.into();
        if let Some(recommendation) = request
            .data
            .get("pick")
            .and_then(|p| p.get("recommendation"))
        {
            pick["recommendation"] = recommendation.clone();
        }
        actor.clone_from(&recorded.actor);
    }
    data["pick"] = pick;
    let mut verdict = event(
        "gate_verdict",
        &started.gate_id,
        gate_attempt(&events, &started.gate_id),
        data,
    );
    verdict.gate_id.clone_from(&started.gate_id);
    verdict.actor = actor;
    crate::run_worker::append_live(c, id, control, verdict).map(|_| ())
}

/// Ask the operator to pick the best attempt, with the judge's pick as a
/// recommendation when the gate has a judge. Only this path waits for the
/// pick; the run goes on with every other branch, as with a verdict.
fn ask(
    gate: &Gate,
    started: &RunEvent,
    input: &RoutedInput,
    attempts: &[i64],
    recommendation: Option<&Choice>,
) -> RunEvent {
    let mut pick = json!({"nodeId":started.node_id,"pickSeq":started.seq,"candidates":attempts});
    if let Some(recommendation) = recommendation {
        pick["recommendation"] = recommendation.data();
    }
    let choices: Vec<_> = attempts.iter().map(i64::to_string).collect();
    let mut request = event(
        "human_input_requested",
        &gate.id,
        0,
        json!({"gateId":gate.id,"nodeId":gate.id,"prompt":gate.criterion,"choices":choices,"requestedBy":gate.id,"inputRef":input,"pick":pick}),
    );
    request.gate_id = gate.id.clone();
    request
}

/// What `advance` did with a pick.
pub(crate) enum PickAction {
    None,
    Acted,
    /// The gate's judge chain picks on a thread of its own.
    Judging(String),
}

/// Go on with the first unsettled pick whose gate is not busy: the only
/// candidate is taken without a picker; otherwise the gate's judge chain
/// picks, on a thread, and a human gate asks the operator.
pub(crate) fn advance(
    context: &crate::run_gate::GateContext<'_>,
    running: &std::collections::HashSet<String>,
) -> io::Result<PickAction> {
    let crate::run_gate::GateContext {
        c,
        id,
        control,
        mission,
        events,
        ..
    } = *context;
    for started in unsettled(events) {
        if running.contains(&format!("gate:{}", started.gate_id)) {
            continue;
        }
        let gate = mission.gate(&started.gate_id).ok_or_else(|| {
            invalid(&format!(
                "pick {} names gate {:?}",
                started.seq, started.gate_id
            ))
        })?;
        let details = details(events, started)?;
        if let [only] = details.as_slice() {
            let choice = Choice {
                attempt: only.attempt,
                reason: format!("the only attempt {} judged", mission.node_name(&gate.id)),
                evidence: Vec::new(),
                decided_by: DECIDED_BY_PARVI.into(),
            };
            resolve(c, id, control, mission, started, &choice, None)?;
            return Ok(PickAction::Acted);
        }
        let input = pick_input(id, mission, gate, started, &details);
        let attempts: Vec<_> = details.iter().map(|detail| detail.attempt).collect();
        if gate.kinds.iter().any(|kind| kind == "formation") {
            if crate::graph::judge_chain(mission, &gate.id).is_empty() {
                return Err(invalid(&format!("gate {:?} has no judge chain", gate.id)));
            }
            spawn_judges(context, gate.clone(), started.clone(), input, attempts);
            return Ok(PickAction::Judging(gate.id.clone()));
        }
        if !gate.kinds.iter().any(|kind| kind == "human") {
            return Err(invalid(&format!(
                "gate {:?} has no judge or human to pick",
                gate.id
            )));
        }
        let request = ask(gate, started, &input, &attempts, None);
        crate::run_worker::append_live(c, id, control, request)?;
        return Ok(PickAction::Acted);
    }
    Ok(PickAction::None)
}

fn spawn_judges(
    context: &crate::run_gate::GateContext<'_>,
    gate: Gate,
    started: RunEvent,
    input: RoutedInput,
    attempts: Vec<i64>,
) {
    let (c, id, mission, req, workspace, control, inputs, done, wake) = (
        context.c.clone(),
        context.id.to_owned(),
        context.mission.clone(),
        context.req.clone(),
        context.workspace.to_path_buf(),
        context.control.clone(),
        context.inputs.to_vec(),
        context.done.clone(),
        context.wake.clone(),
    );
    std::thread::spawn(move || {
        let judged = Judging {
            c: &c,
            id: &id,
            mission: &mission,
            req: &req,
            workspace: &workspace,
            control: &control,
            inputs: &inputs,
        };
        let result = judged.pick(&gate, &started, input, &attempts);
        let _ = done.send(crate::run_scheduler::Finished::Pick(
            gate.id.clone(),
            result,
        ));
        let _ = wake.send(());
    });
}

struct Judging<'a> {
    c: &'a Coordinator,
    id: &'a str,
    mission: &'a Mission,
    req: &'a RunStart,
    workspace: &'a std::path::Path,
    control: &'a Control,
    inputs: &'a [crate::RunInput],
}

impl Judging<'_> {
    /// Run the gate's judge chain to pick: every judge reads the attempts
    /// side by side, the judges before the last recommend and the last picks.
    /// Judge outputs the ledger holds since `pick_started` are reused, except
    /// the final judge's after its malformed pick blocked the run.
    fn pick(
        &self,
        gate: &Gate,
        started: &RunEvent,
        input: RoutedInput,
        attempts: &[i64],
    ) -> io::Result<()> {
        let Judging {
            c,
            id,
            mission,
            control,
            ..
        } = *self;
        let chain = crate::graph::judge_chain(mission, &gate.id);
        let ledger = c.ledger(id)?.read()?;
        let invalidated = ledger.iter().any(|e| {
            e.seq > started.seq
                && e.kind == "run_blocked"
                && e.gate_id == gate.id
                && e.data.get("code").and_then(Value::as_str) == Some(INVALID_JUDGE_PICK)
        });
        let mut current = input;
        let count = chain.len();
        let mut final_text = String::new();
        for (index, judge) in chain.into_iter().enumerate() {
            let last = index + 1 == count;
            let events = c.ledger(id)?.read()?;
            let previous = (!(invalidated && last))
                .then(|| {
                    events.iter().rev().find(|e| {
                        e.seq > started.seq
                            && e.kind == "node_output"
                            && e.node_id == judge.id
                            && e.data.get("reason").and_then(Value::as_str) == Some("judge")
                    })
                })
                .flatten();
            let (text, outputs) = if let Some(output) = previous {
                (
                    output
                        .data
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    output.data.get("outputs").cloned().unwrap_or_default(),
                )
            } else {
                let attempt = (events.iter())
                    .filter(|e| e.kind == "node_started" && e.node_id == judge.id)
                    .map(|e| e.attempt)
                    .max()
                    .unwrap_or(0)
                    + 1;
                let result = crate::run_job::execute_formation(
                    c,
                    id,
                    mission,
                    self.req,
                    self.workspace,
                    control,
                    judge.id.clone(),
                    attempt,
                    vec![serde_json::to_value(&current)?],
                    current.text.clone(),
                    self.inputs,
                    "judge",
                    last,
                    None,
                    attempts.to_vec(),
                )?;
                crate::run_judges::record_output(
                    c,
                    id,
                    self.req,
                    self.workspace,
                    control,
                    &judge.id,
                    attempt,
                    &result,
                )?;
                (result.text.clone(), Value::Object(result.outputs))
            };
            if last {
                final_text = text;
                break;
            }
            let recommendation = (judge.outputs.first())
                .and_then(|port| outputs.get(&port.id))
                .and_then(|payload| {
                    let text = payload
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim();
                    let kind = payload.get("kind").and_then(Value::as_str).unwrap_or("");
                    let reference = payload.get("ref").and_then(Value::as_str).unwrap_or("");
                    if !text.is_empty() {
                        Some(text.to_owned())
                    } else if (kind == "file" || kind == "folder") && !reference.is_empty() {
                        Some(format!("{kind}: {reference}"))
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| text.trim().to_owned());
            current.text += &format!("\nrecommendation from {}:\n{recommendation}\n", judge.title);
        }
        let judge = crate::graph::judge_chain(mission, &gate.id)
            .last()
            .map(|judge| judge.id.clone())
            .unwrap_or_default();
        match parse_pick(&final_text, attempts) {
            Ok(mut choice) => {
                choice.decided_by = judge;
                if gate.kinds.iter().any(|kind| kind == "human") {
                    let events = c.ledger(id)?.read()?;
                    let details = details(&events, started)?;
                    let input = pick_input(id, mission, gate, started, &details);
                    let request = ask(gate, started, &input, attempts, Some(&choice));
                    crate::run_worker::append_live(c, id, control, request).map(|_| ())
                } else {
                    resolve(c, id, control, mission, started, &choice, None)
                }
            }
            Err(error) => {
                let events = c.ledger(id)?.read()?;
                let mut failed = event(
                    "judge_attempt_failed",
                    &gate.id,
                    gate_attempt(&events, &gate.id),
                    json!({"code":INVALID_JUDGE_PICK,"reason":error}),
                );
                failed.gate_id = gate.id.clone();
                crate::run_worker::append_live(c, id, control, failed)?;
                Err(io::Error::other(format!(
                    "{INVALID_JUDGE_PICK}[{}]: {error}",
                    gate.id
                )))
            }
        }
    }
}

/// Route the operator's recorded pick of a pending pick request.
pub(crate) fn route_human(
    c: &Coordinator,
    id: &str,
    control: &Control,
    mission: &Mission,
    events: &[RunEvent],
    request: &RunEvent,
    recorded: &RunEvent,
) -> io::Result<()> {
    let seq = pick_seq(request);
    let started = (events.iter())
        .find(|e| {
            e.seq == seq
                && e.kind == PICK_STARTED
                && e.gate_id == request.gate_id
                && seq < request.seq
        })
        .ok_or_else(|| {
            invalid(&format!(
                "gate {:?} pick request names invalid pick {seq}",
                request.gate_id
            ))
        })?;
    let choice = Choice {
        attempt: (recorded.data.get("pick"))
            .and_then(|pick| pick.get("attempt"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        reason: text(&Value::Object(recorded.data.clone()), "reason").into(),
        evidence: Vec::new(),
        decided_by: text(&Value::Object(recorded.data.clone()), "decidedBy").into(),
    };
    resolve(
        c,
        id,
        control,
        mission,
        started,
        &choice,
        Some((request, recorded)),
    )
}

/// The operator's pick on a gate's pending pick.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct HumanPick {
    /// The exact pending request picked on.
    pub requested_seq: u64,
    pub attempt: i64,
    pub reason: String,
    /// The slot ID of the seat that typed a pick the actor made.
    pub relayed_by: String,
    pub actor: String,
}

/// The pick a pending human request asks for, with the judge's pick as a
/// recommendation where the gate has a judge.
pub(crate) fn request_candidates(request: &RunEvent) -> Option<Vec<i64>> {
    let pick = request.data.get("pick")?;
    Some(
        (pick.get("candidates").and_then(Value::as_array).into_iter())
            .flatten()
            .filter_map(Value::as_i64)
            .collect(),
    )
}

impl Coordinator {
    /// Record the operator's pick on a gate's pending pick. Like a verdict it
    /// routes nothing: the run's worker routes it, so a pick is accepted
    /// while other seats work, and on a blocked run it waits for the resume.
    pub fn record_pick(
        &self,
        run: &str,
        gate: &str,
        pick: HumanPick,
    ) -> Result<RunProjection, crate::Error> {
        (|| -> io::Result<RunProjection> {
        let relay = pick.relayed_by.as_bytes();
        if pick.requested_seq == 0
            || (!relay.is_empty()
                && (relay.len() > 64
                    || !relay[0].is_ascii_alphanumeric()
                    || !relay
                        .iter()
                        .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "requestedSeq, an attempt and valid relayedBy required",
            ));
        }
        let control = self.inner.controls.lock().unwrap().get(run).cloned();
        let guard = control.as_ref().map(|control| control.done.lock().unwrap());
        if control
            .as_ref()
            .is_some_and(|control| control.canceled.load(std::sync::atomic::Ordering::Acquire))
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "run is final or aborting",
            ));
        }
        let actor = if pick.actor.is_empty() {
            "human:operator".to_owned()
        } else {
            pick.actor.clone()
        };
        let mut data = json!({"gateId":gate,"nodeId":gate,"verdict":"pass","reason":pick.reason,"requestedSeq":pick.requested_seq,"decidedBy":actor,"pick":{"attempt":pick.attempt}});
        if !pick.relayed_by.is_empty() {
            data["relayedBy"] = pick.relayed_by.clone().into();
        }
        let event = RunEvent {
            kind: "human_verdict_recorded".into(),
            gate_id: gate.into(),
            node_id: gate.into(),
            timestamp: crate::run_storage::now(),
            actor: actor.clone(),
            data: data.as_object().unwrap().clone(),
            ..Default::default()
        };
        let appended = self.ledger(run)?.append_checked(event, |events| {
            let projection = RunProjection::from_events(events, run)?;
            if projection.r#final
                || !projection.waiting_gates.iter().any(|pending| {
                    pending.gate_id == gate && pending.requested_seq == pick.requested_seq
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "human gate request is no longer pending",
                ));
            }
            let request = (events.iter())
                .find(|e| e.seq == pick.requested_seq)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "human gate request"))?;
            let Some(candidates) = request_candidates(request) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "this gate waits for a verdict, not a pick: answer it with gate approve or gate reject",
                ));
            };
            if !candidates.contains(&pick.attempt) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "not a candidate attempt: attempt {}; pick one of {}",
                        pick.attempt,
                        attempt_words(&candidates)
                    ),
                ));
            }
            Ok(())
        })?;
        drop(guard);
        self.changed(run, appended.seq);
        if let Some(control) = self.inner.controls.lock().unwrap().get(run)
            && let Some(wake) = control.wake.lock().unwrap().as_ref()
        {
            let _ = wake.send(());
        }
        self.run(run).map_err(crate::error::to_io)
     })().map_err(crate::error::io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_judge_pick_is_one_fenced_block_naming_a_candidate() {
        let block = |body: &str| format!("```parvi-pick\n{body}\n```");
        let choice = parse_pick(
            &block(r#"{"attempt":2,"reason":"tests pass","evidence":["retry"]}"#),
            &[1, 2, 3],
        )
        .unwrap();
        assert_eq!((choice.attempt, choice.reason.as_str()), (2, "tests pass"));
        assert_eq!(choice.evidence, ["retry"]);
        assert!(
            parse_pick(
                &block(r#"{"attempt":4,"reason":"","evidence":[]}"#),
                &[1, 2, 3]
            )
            .unwrap_err()
            .contains("one of attempts 1, 2 or 3")
        );
        assert!(
            parse_pick(&block(r#"{"attempt":1,"reason":""}"#), &[1])
                .unwrap_err()
                .contains("missing evidence")
        );
        assert!(
            parse_pick(
                &block(r#"{"attempt":1,"reason":"","evidence":[],"x":1}"#),
                &[1]
            )
            .unwrap_err()
            .contains("extra key")
        );
        assert!(parse_pick("attempt 2 is best", &[1, 2]).is_err());
    }

    #[test]
    fn contracts_name_the_candidates() {
        assert!(contract(true, &[1, 2, 3]).contains("attempt must be one of attempts 1, 2 or 3"));
        assert!(contract(false, &[1, 2]).starts_with("judge recommend contract:"));
    }
}
