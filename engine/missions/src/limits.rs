//! What a run's Limit cards allow, counted from its ledger, so replay and
//! every reader agree (Archon `run_limit_block.go`, `token_usage.go`).
//!
//! Rounds count a step's starts, or every step start of the run for a card on
//! the Input card; a start a restart cut short is not a round. Time counts
//! wall time while the covered step, or any step for the mission, has an
//! attempt running and the run is not blocked. Tokens count the `token_usage`
//! the executor recorded for the step, or for every step. A grant, recorded
//! on the `run_resumed` that gives it, adds one more allowance: a round, or
//! the card's time or tokens again.
use crate::RunEvent;
use parvi_mission_model::{Limit, Mission};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{Duration, SystemTime};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(crate) const ROUNDS: &str = "rounds";
pub(crate) const TIME: &str = "time";
pub(crate) const TOKENS: &str = "tokens";
/// The code of the error, block or failure a spent limit records.
pub(crate) const LIMIT_REACHED: &str = "limit_reached";
pub(crate) const WHEN_SPENT_BEST: &str = "best";
pub(crate) const WHEN_SPENT_FAIL: &str = "fail";

/// A Limit card's knob as the run used it: `used` of `max`, `max` counting
/// the card's own value and every grant. Time is in whole seconds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitUse {
    pub kind: String,
    pub limit_id: String,
    /// The step the card covers, or the Input card for the mission.
    pub node_id: String,
    pub used: i64,
    pub max: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub granted: i64,
}
fn is_zero(n: &i64) -> bool {
    *n == 0
}
impl LimitUse {
    pub(crate) fn spent(&self) -> bool {
        self.used >= self.max
    }
    fn left(&self) -> i64 {
        self.max - self.used
    }
    /// The use a block, failure or error recorded, if any.
    pub(crate) fn recorded(event: &RunEvent) -> Option<Self> {
        serde_json::from_value(event.data.get("limit")?.clone()).ok()
    }
}

/// A time card's warning for the seats of an attempt: pasted once into each
/// seat at `at`. The mission's card warns only the seats working when it
/// first fires, once in the whole run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitWarning {
    pub limit_id: String,
    pub mission: bool,
    pub at: SystemTime,
    pub text: String,
}

pub(crate) fn card<'a>(mission: &'a Mission, limit_id: &str) -> Option<&'a Limit> {
    mission.limits.iter().find(|limit| limit.id == limit_id)
}
fn covering<'a>(mission: &'a Mission, node: &str) -> Option<&'a Limit> {
    mission.limits.iter().find(|limit| limit.target == node)
}
fn is_mission_card(mission: &Mission, limit: &Limit) -> bool {
    mission.has_input_card(&limit.target)
}
/// The cards that cover a step: its own, then the mission's.
fn covering_cards<'a>(mission: &'a Mission, node: &str) -> Vec<&'a Limit> {
    let own = covering(mission, node);
    let whole = (mission.input_cards.iter()).find_map(|input| covering(mission, &input.id));
    own.into_iter().chain(whole).collect()
}

fn text<'a>(event: &'a RunEvent, key: &str) -> &'a str {
    event.data.get(key).and_then(|v| v.as_str()).unwrap_or("")
}
fn number(value: Option<&serde_json::Value>) -> i64 {
    value.and_then(|v| v.as_i64()).unwrap_or(0)
}

/// What grants added to a card's knob.
pub(crate) fn granted(events: &[RunEvent], limit_id: &str, kind: &str) -> i64 {
    (events.iter())
        .filter(|event| event.kind == "run_resumed")
        .filter_map(|event| event.data.get("grant"))
        .filter(|grant| grant["limitId"] == limit_id && grant["kind"] == kind)
        .map(|grant| number(grant.get("amount")))
        .sum()
}

fn use_of(
    events: &[RunEvent],
    limit: &Limit,
    kind: &str,
    knob: Option<i64>,
    used: i64,
) -> Option<LimitUse> {
    let knob = knob.filter(|value| *value > 0)?;
    let granted = granted(events, &limit.id, kind);
    Some(LimitUse {
        kind: kind.into(),
        limit_id: limit.id.clone(),
        node_id: limit.target.clone(),
        used,
        max: knob + granted,
        granted,
    })
}

/// A stopped coordinator's mark: the error a restart records after a crash,
/// or the block a shutdown records.
fn coordinator_stopped(event: &RunEvent) -> bool {
    let code = text(event, "code");
    (event.kind == "error" && code == "coordinator_interrupted")
        || (event.kind == "run_blocked" && code == "coordinator_shutdown")
}

/// Whether the step started at `index` never produced its output because the
/// coordinator stopped, so the run started it again: a run reaches the same
/// limits wherever a restart falls.
fn cut_short_by_restart(events: &[RunEvent], index: usize) -> bool {
    let node = &events[index].node_id;
    let mut interrupted = false;
    for event in &events[index + 1..] {
        if &event.node_id == node && event.kind == "node_started" {
            return interrupted;
        }
        if &event.node_id == node && event.kind == "node_output" {
            return false;
        }
        interrupted |= coordinator_stopped(event);
    }
    interrupted
}

fn rounds_use(mission: &Mission, events: &[RunEvent], limit: &Limit) -> Option<LimitUse> {
    let whole = is_mission_card(mission, limit);
    let used = (events.iter().enumerate())
        .filter(|(_, event)| {
            event.kind == "node_started"
                && text(event, "nodeKind") == "formation"
                && (whole || event.node_id == limit.target)
        })
        .filter(|(index, _)| !cut_short_by_restart(events, *index))
        .count() as i64;
    use_of(events, limit, ROUNDS, limit.rounds, used)
}

fn parse_time(timestamp: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(timestamp, &Rfc3339).ok()
}

/// The whole seconds a card's time has counted by `now`: while the covered
/// step has an attempt running, or for the mission while any step has, and
/// the run is not blocked. An attempt runs from its `node_started` to its
/// output, its own error, its abandoned dispatch or the run's end.
fn time_used(mission: &Mission, events: &[RunEvent], limit: &Limit, now: OffsetDateTime) -> i64 {
    let whole = is_mission_card(mission, limit);
    let mut open = HashSet::new();
    let mut blocked = false;
    let mut used = time::Duration::ZERO;
    let mut last: Option<OffsetDateTime> = None;
    let count = |last: &mut Option<OffsetDateTime>,
                 used: &mut time::Duration,
                 at: Option<OffsetDateTime>,
                 counting: bool| {
        if let (Some(from), Some(to)) = (*last, at)
            && counting
            && to > from
        {
            *used += to - from;
        }
        if at.is_some() {
            *last = at;
        }
    };
    for event in events {
        if event.kind == "error" && text(event, "code") == "coordinator_interrupted" {
            // The time before a crash's restart was mostly downtime.
            last = None;
        }
        let counting = !open.is_empty() && !blocked;
        count(&mut last, &mut used, parse_time(&event.timestamp), counting);
        match event.kind.as_str() {
            "node_started"
                if (whole || event.node_id == limit.target)
                    && text(event, "nodeKind") == "formation" =>
            {
                open.insert(event.node_id.clone());
            }
            "node_output" | "error" => {
                open.remove(&event.node_id);
            }
            "slot_result" if text(event, "status") == "abandoned" => {
                open.remove(&event.node_id);
            }
            "run_blocked" => blocked = true,
            "run_resumed" => blocked = false,
            "run_succeeded" | "run_failed" | "run_canceled" | "run_picked" => open.clear(),
            _ => {}
        }
    }
    let counting = !open.is_empty() && !blocked;
    count(&mut last, &mut used, Some(now), counting);
    used.whole_seconds()
}

fn time_use(
    mission: &Mission,
    events: &[RunEvent],
    limit: &Limit,
    now: OffsetDateTime,
) -> Option<LimitUse> {
    let used = time_used(mission, events, limit, now);
    use_of(events, limit, TIME, limit.seconds, used)
}

fn tokens_use(mission: &Mission, events: &[RunEvent], limit: &Limit) -> Option<LimitUse> {
    let whole = is_mission_card(mission, limit);
    let used = (events.iter())
        .filter(|event| event.kind == "token_usage" && (whole || event.node_id == limit.target))
        .map(|event| number(event.data.get("tokens")))
        .sum();
    use_of(events, limit, TOKENS, limit.tokens, used)
}

/// All configured limits covering a step, measured by the same rules as admission.
pub(crate) fn uses_for_step(
    mission: &Mission,
    events: &[RunEvent],
    node: &str,
    now: OffsetDateTime,
) -> Vec<LimitUse> {
    covering_cards(mission, node)
        .into_iter()
        .flat_map(|limit| {
            [
                rounds_use(mission, events, limit),
                time_use(mission, events, limit, now),
                tokens_use(mission, events, limit),
            ]
        })
        .flatten()
        .collect()
}

/// The limit that starting `node` now would pass: the step's own card first,
/// then the mission's, rounds before time before tokens. `None` means it may
/// start.
pub(crate) fn spent_before(
    mission: &Mission,
    events: &[RunEvent],
    node: &str,
    now: OffsetDateTime,
) -> Option<LimitUse> {
    covering_cards(mission, node).into_iter().find_map(|limit| {
        [
            rounds_use(mission, events, limit),
            time_use(mission, events, limit, now),
            tokens_use(mission, events, limit),
        ]
        .into_iter()
        .flatten()
        .find(LimitUse::spent)
    })
}

/// What the time cards covering a step allow it from `now`: the use of the
/// card that runs out first, and the warnings its seats get.
pub(crate) fn time_budget(
    mission: &Mission,
    events: &[RunEvent],
    node: &str,
    now: OffsetDateTime,
) -> (Option<LimitUse>, Vec<LimitWarning>) {
    let mut first: Option<LimitUse> = None;
    let mut warnings = Vec::new();
    let clock = SystemTime::now();
    for limit in covering_cards(mission, node) {
        let Some(use_) = time_use(mission, events, limit, now) else {
            continue;
        };
        let left = use_.left();
        if first.as_ref().is_none_or(|first| left < first.left()) {
            first = Some(use_);
        }
        let whole = is_mission_card(mission, limit);
        let Some(warn) = limit.warn_seconds.filter(|warn| *warn > 0) else {
            continue;
        };
        if whole && warned(events, &limit.id) {
            continue;
        }
        let warn_left = left.min(warn).max(0);
        warnings.push(LimitWarning {
            limit_id: limit.id.clone(),
            mission: whole,
            at: clock + Duration::from_secs((left - warn_left).max(0) as u64),
            text: warning_text(mission, limit, warn_left),
        });
    }
    (first, warnings)
}

/// The tokens card covering a step that has the fewest left: the step's own
/// or the mission's.
pub(crate) fn token_budget(mission: &Mission, events: &[RunEvent], node: &str) -> Option<LimitUse> {
    (covering_cards(mission, node).into_iter())
        .filter_map(|limit| tokens_use(mission, events, limit))
        .min_by_key(LimitUse::left)
}

/// The mission card's tokens, which every attempt working at once shares.
pub(crate) fn mission_tokens(mission: &Mission, events: &[RunEvent]) -> Option<LimitUse> {
    let limit = (mission.input_cards.iter()).find_map(|input| covering(mission, &input.id))?;
    tokens_use(mission, events, limit)
}

fn warned(events: &[RunEvent], limit_id: &str) -> bool {
    (events.iter()).any(|event| event.kind == "limit_warning" && text(event, "limitId") == limit_id)
}

/// Whether the ledger refuses a seat's warning: each seat of an attempt gets
/// a card's warning once, and the mission's card warns only the attempts that
/// were working when it first fired.
pub(crate) fn warning_refused(events: &[RunEvent], warning: &RunEvent) -> bool {
    let limit = text(warning, "limitId");
    let started = (events.iter())
        .filter(|e| {
            e.kind == "node_started" && e.node_id == warning.node_id && e.attempt == warning.attempt
        })
        .map(|e| e.seq)
        .next_back()
        .unwrap_or(0);
    let mut first = 0;
    for event in events {
        if event.kind != "limit_warning" || text(event, "limitId") != limit {
            continue;
        }
        if first == 0 {
            first = event.seq;
        }
        if event.node_id == warning.node_id
            && event.attempt == warning.attempt
            && event.slot_id == warning.slot_id
        {
            return true;
        }
    }
    warning.data.get("mission").and_then(|v| v.as_bool()) == Some(true)
        && (started == 0 || (first > 0 && started > first))
}

fn warning_text(mission: &Mission, limit: &Limit, left: i64) -> String {
    let whose = if is_mission_card(mission, limit) {
        "the mission's"
    } else {
        "this step's"
    };
    let ending = match limit.when_spent.as_str() {
        WHEN_SPENT_FAIL => "fails, keeping the attempts as evidence",
        WHEN_SPENT_BEST => "picks the best judged attempt, or waits if none is eligible",
        _ => "waits for the operator",
    };
    format!(
        "Parvi: {} left of {whose} working time (Limit card {}). When it runs out the step stops and the run {ending}. Finish your output now.",
        duration_words(left),
        mission.node_name(&limit.id)
    )
}

/// What a spent limit used, plainly: "Review used 3 of 3 rounds", "The
/// mission used 20 of 20 rounds, 1 of them granted", "Review used 30 min of
/// 30 min", "Review used 51,230 of 50,000 tokens".
pub(crate) fn reason(mission: &Mission, use_: &LimitUse) -> String {
    let who = if mission.has_input_card(&use_.node_id) {
        "The mission"
    } else {
        mission.node_name(&use_.node_id)
    };
    match use_.kind.as_str() {
        TIME => {
            let mut reason = format!(
                "{who} used {} of {}",
                duration_words(use_.used),
                duration_words(use_.max)
            );
            if use_.granted > 0 {
                reason += &format!(", {} of it granted", duration_words(use_.granted));
            }
            reason
        }
        TOKENS => {
            let mut reason = format!(
                "{who} used {} of {}",
                thousands(use_.used),
                token_words(use_.max)
            );
            if use_.granted > 0 {
                reason += &format!(", {} of them granted", thousands(use_.granted));
            }
            reason
        }
        _ => {
            let noun = if use_.max == 1 { "round" } else { "rounds" };
            let mut reason = format!("{who} used {} of {} {noun}", use_.used, use_.max);
            if use_.granted > 0 {
                reason += &format!(", {} of them granted", use_.granted);
            }
            reason
        }
    }
}

/// What a grant gives a spent limit: one round, or the card's time or tokens
/// again. `None` when the card no longer sets that knob.
pub(crate) fn grant_amount(mission: &Mission, use_: &LimitUse) -> Option<i64> {
    let limit = card(mission, &use_.limit_id)?;
    match use_.kind.as_str() {
        ROUNDS => Some(1),
        TIME => limit.seconds.filter(|value| *value > 0),
        TOKENS => limit.tokens.filter(|value| *value > 0),
        _ => None,
    }
}

/// Whole seconds said plainly: "45 s", "5 min", "1 h 30 min".
pub(crate) fn duration_words(seconds: i64) -> String {
    if seconds < 60 {
        return format!("{seconds} s");
    }
    let parts = [
        (seconds / 3600, "h"),
        (seconds % 3600 / 60, "min"),
        (seconds % 60, "s"),
    ];
    let words: Vec<_> = (parts.iter())
        .filter(|(count, _)| *count > 0)
        .map(|(count, unit)| format!("{count} {unit}"))
        .collect();
    words.join(" ")
}

fn thousands(count: i64) -> String {
    let digits = count.unsigned_abs().to_string();
    let mut words = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            words.push(',');
        }
        words.push(digit);
    }
    if count < 0 {
        format!("-{words}")
    } else {
        words
    }
}

/// A token count said plainly: "1 token", "50,000 tokens".
fn token_words(count: i64) -> String {
    let noun = if count == 1 { "token" } else { "tokens" };
    format!("{} {noun}", thousands(count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mission(cards: &str) -> Mission {
        Mission::parse(&format!(
            "schema = 1\nid = \"m\"\nslug = \"m\"\ntitle = \"M\"\nrev = 1\n[[inputCard]]\nid = \"inp\"\ntitle = \"In\"\n[[formation]]\nid = \"work\"\ntype = \"solo\"\ntitle = \"Review\"\n{cards}"
        ))
        .unwrap()
    }
    fn event(seq: u64, ts: &str, kind: &str, node: &str, data: serde_json::Value) -> RunEvent {
        RunEvent {
            seq,
            timestamp: ts.into(),
            kind: kind.into(),
            node_id: node.into(),
            data: data.as_object().unwrap().clone(),
            ..Default::default()
        }
    }
    fn at(ts: &str) -> OffsetDateTime {
        parse_time(ts).unwrap()
    }

    #[test]
    fn words_read_as_the_engine_says_them() {
        assert_eq!(duration_words(45), "45 s");
        assert_eq!(duration_words(300), "5 min");
        assert_eq!(duration_words(5400), "1 h 30 min");
        assert_eq!(duration_words(90), "1 min 30 s");
        assert_eq!(token_words(1), "1 token");
        assert_eq!(token_words(50_000), "50,000 tokens");
        assert_eq!(token_words(1_234_567), "1,234,567 tokens");
        let m =
            mission("[[limit]]\nid = \"lim\"\ntitle = \"Cap\"\ntarget = \"inp\"\nrounds = 20\n");
        let use_ = LimitUse {
            kind: ROUNDS.into(),
            limit_id: "lim".into(),
            node_id: "inp".into(),
            used: 20,
            max: 20,
            granted: 1,
        };
        assert_eq!(
            reason(&m, &use_),
            "The mission used 20 of 20 rounds, 1 of them granted"
        );
        let tokens = LimitUse {
            kind: TOKENS.into(),
            node_id: "work".into(),
            used: 51_230,
            max: 50_000,
            granted: 0,
            ..use_
        };
        assert_eq!(reason(&m, &tokens), "Review used 51,230 of 50,000 tokens");
    }

    #[test]
    fn rounds_count_starts_but_not_one_a_restart_cut_short() {
        let m =
            mission("[[limit]]\nid = \"lim\"\ntitle = \"Cap\"\ntarget = \"work\"\nrounds = 2\n");
        let t = "2026-01-01T00:00:00Z";
        let started = json!({"nodeKind":"formation"});
        let mut events = vec![
            event(1, t, "node_started", "work", started.clone()),
            event(2, t, "error", "", json!({"code":"coordinator_interrupted"})),
            event(3, t, "node_started", "work", started.clone()),
            event(4, t, "node_output", "work", json!({})),
        ];
        let now = at(t);
        assert_eq!(spent_before(&m, &events, "work", now), None);
        events.push(event(5, t, "node_started", "work", started));
        let spent = spent_before(&m, &events, "work", now).unwrap();
        assert_eq!((spent.used, spent.max), (2, 2));
        events.push(event(
            6,
            t,
            "run_resumed",
            "",
            json!({"grant":{"limitId":"lim","kind":"rounds","amount":1}}),
        ));
        assert_eq!(spent_before(&m, &events, "work", now), None);
    }

    #[test]
    fn time_counts_only_while_the_step_runs_and_the_run_is_not_blocked() {
        let m = mission(
            "[[limit]]\nid = \"lim\"\ntitle = \"Clock\"\ntarget = \"work\"\nseconds = 60\nwarnSeconds = 10\n",
        );
        let events = vec![
            event(
                1,
                "2026-01-01T00:00:00Z",
                "node_started",
                "work",
                json!({"nodeKind":"formation"}),
            ),
            event(2, "2026-01-01T00:00:20Z", "node_output", "work", json!({})),
            event(
                3,
                "2026-01-01T00:05:00Z",
                "node_started",
                "work",
                json!({"nodeKind":"formation"}),
            ),
            event(4, "2026-01-01T00:05:10Z", "run_blocked", "", json!({})),
            event(5, "2026-01-01T00:09:00Z", "run_resumed", "", json!({})),
        ];
        let limit = card(&m, "lim").unwrap();
        assert_eq!(
            time_used(&m, &events, limit, at("2026-01-01T00:09:05Z")),
            35
        );
        let (first, warnings) = time_budget(&m, &events, "work", at("2026-01-01T00:09:05Z"));
        assert_eq!(first.unwrap().used, 35);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0]
                .text
                .contains("10 s left of this step's working time")
        );
        assert!(warnings[0].text.contains("waits for the operator"));
    }

    #[test]
    fn tokens_count_recorded_usage_and_the_mission_card_counts_every_step() {
        let m = mission(
            "[[limit]]\nid = \"lim\"\ntitle = \"Budget\"\ntarget = \"inp\"\ntokens = 100\n",
        );
        let t = "2026-01-01T00:00:00Z";
        let events = vec![
            event(1, t, "token_usage", "work", json!({"tokens":60})),
            event(2, t, "token_usage", "other", json!({"tokens":50})),
        ];
        let spent = spent_before(&m, &events, "work", at(t)).unwrap();
        assert_eq!(
            (spent.kind.as_str(), spent.used, spent.max),
            (TOKENS, 110, 100)
        );
        assert_eq!(token_budget(&m, &events, "work").unwrap().used, 110);
    }

    #[test]
    fn a_mission_warning_reaches_only_the_attempts_working_when_it_fired() {
        let started = |seq, node: &str| {
            let mut e = event(seq, "", "node_started", node, json!({}));
            e.attempt = 1;
            e
        };
        let warning = |seq, node: &str, slot: &str| {
            let mut e = event(
                seq,
                "",
                "limit_warning",
                node,
                json!({"limitId":"lim","mission":true}),
            );
            e.attempt = 1;
            e.slot_id = slot.into();
            e
        };
        let events = vec![
            started(1, "a"),
            started(2, "b"),
            warning(3, "a", "s"),
            started(4, "c"),
        ];
        assert!(warning_refused(&events, &warning(5, "a", "s")));
        assert!(!warning_refused(&events, &warning(5, "b", "s")));
        assert!(warning_refused(&events, &warning(5, "c", "s")));
    }
}
