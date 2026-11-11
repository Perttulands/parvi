//! What a spent Limit card does to a run (Archon `run_stop.go`
//! `stopAtLimit`, `run_scheduler.go` `pickOrStop`): `fail` ends the run,
//! keeping its attempts as evidence; `best` picks the best judged attempt
//! when the step's gate judged one since its last pick; anything else
//! records the limit and blocks the run there for a grant.
use crate::coordinator::Control;
use crate::limits::{self, LIMIT_REACHED, LimitUse, WHEN_SPENT_BEST, WHEN_SPENT_FAIL};
use crate::run_worker::event;
use crate::{Coordinator, Mission, RunEvent};
use serde_json::json;
use std::io;

/// How a step met a spent limit: before it started, or while it worked
/// (`cut_off`), when the ledger records why the attempt stopped and it is
/// never a pick candidate.
#[derive(Debug)]
pub(crate) struct LimitReached {
    pub node: String,
    pub use_: LimitUse,
    pub cut_off: bool,
}
impl std::fmt::Display for LimitReached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{LIMIT_REACHED}: {} {} {} of {}",
            self.node, self.use_.kind, self.use_.used, self.use_.max
        )
    }
}
impl std::error::Error for LimitReached {}

pub(crate) fn reached(node: &str, use_: LimitUse, cut_off: bool) -> io::Error {
    io::Error::other(LimitReached {
        node: node.into(),
        use_,
        cut_off,
    })
}
pub(crate) fn of(error: &io::Error) -> Option<&LimitReached> {
    error.get_ref()?.downcast_ref()
}

/// Why the run stops at a limit, kept until its in-flight work settles.
#[derive(Debug)]
pub(crate) struct LimitStop {
    pub node: String,
    pub use_: LimitUse,
    pub fail: bool,
}

/// Decide what a spent limit does to `node`: start a pick, or return the
/// stop the run leaves once nothing runs.
pub(crate) fn pick_or_stop(
    c: &Coordinator,
    id: &str,
    control: &Control,
    mission: &Mission,
    reached: &LimitReached,
) -> io::Result<Option<LimitStop>> {
    let LimitReached {
        node,
        use_,
        cut_off,
    } = reached;
    let events = c.ledger(id)?.read()?;
    let when_spent = limits::card(mission, &use_.limit_id)
        .map(|card| card.when_spent.as_str())
        .unwrap_or("");
    if when_spent == WHEN_SPENT_BEST
        && let Some(pick) = crate::run_pick::pick_for(mission, &events, node, use_)
        && crate::run_pick::judged_since_last_pick(&events, node, &pick.candidates)
    {
        if *cut_off {
            append_limit_error(c, id, control, mission, node, use_)?;
        }
        crate::run_pick::start(c, id, control, mission, node, &pick, use_, "whenSpent")?;
        return Ok(None);
    }
    let fail = when_spent == WHEN_SPENT_FAIL;
    if !fail {
        append_limit_error(c, id, control, mission, node, use_)?;
    }
    Ok(Some(LimitStop {
        node: node.clone(),
        use_: use_.clone(),
        fail,
    }))
}

pub(crate) fn append_limit_error(
    c: &Coordinator,
    id: &str,
    control: &Control,
    mission: &Mission,
    node: &str,
    use_: &LimitUse,
) -> io::Result<()> {
    let reason = limits::reason(mission, use_);
    crate::run_worker::append_live(
        c,
        id,
        control,
        event(
            "error",
            node,
            0,
            json!({"code":LIMIT_REACHED,"message":reason,"reason":reason,"boundary":"limits","nodeId":node,"recoverable":true,"limit":use_}),
        ),
    )
    .map(|_| ())
}

/// Record every dispatch an attempt the limit cut short left unresolved as
/// abandoned, so a resume runs the step again rather than reattaching it.
pub(crate) fn abandon_cut_off(
    c: &Coordinator,
    id: &str,
    control: &Control,
    node: &str,
    attempt: i64,
) -> io::Result<()> {
    let events = c.ledger(id)?.read()?;
    for dispatch in crate::run_open_dispatch::unresolved(&events) {
        if dispatch.node_id != node || dispatch.attempt != attempt {
            continue;
        }
        let mut result = event(
            "slot_result",
            node,
            attempt,
            json!({"dispatchId":dispatch.data.get("dispatchId"),"nodeId":node,"slotId":dispatch.slot_id,"status":"abandoned","reason":format!("attempt {attempt} was cut short by its Limit card")}),
        );
        result.slot_id.clone_from(&dispatch.slot_id);
        crate::run_worker::append_live(c, id, control, result)?;
    }
    Ok(())
}

/// The event a run stopped at a limit ends its epoch with: a failure for a
/// card whose whenSpent is fail, otherwise a block a grant, or where the
/// step can pick, a grant or a pick, resumes.
pub(crate) fn final_event(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    stop: &LimitStop,
) -> io::Result<RunEvent> {
    let reason = limits::reason(mission, &stop.use_);
    if stop.fail {
        crate::run_left_seats::end(c, id, None, "run_final")?;
        return Ok(RunEvent {
            kind: "run_failed".into(),
            actor: "archond".into(),
            node_id: stop.node.clone(),
            timestamp: crate::run_storage::now(),
            data: json!({"code":LIMIT_REACHED,"reason":reason,"limit":stop.use_,"final":true})
                .as_object()
                .unwrap()
                .clone(),
            ..Default::default()
        });
    }
    let events = c.ledger(id)?.read()?;
    let policy = if crate::run_pick::pick_for(mission, &events, &stop.node, &stop.use_).is_some() {
        "grant_or_pick"
    } else {
        "grant"
    };
    Ok(RunEvent {
        kind: "run_blocked".into(),
        node_id: stop.node.clone(),
        timestamp: crate::run_storage::now(),
        data: json!({"code":LIMIT_REACHED,"reason":reason,"limit":stop.use_,"blockedNodeId":stop.node,"blockedGateId":"","resumeAllowed":true,"resumePolicy":policy,"openDispatches":[],"nextEpoch":1})
            .as_object()
            .unwrap()
            .clone(),
        ..Default::default()
    })
}

/// Why a resume, granting or picking as asked, cannot continue a block whose
/// resume policy is `policy`. A spent limit resumes only with a grant, or,
/// where the step can pick, with a grant or a pick; nothing else takes either.
pub(crate) fn resume_refusal(policy: &str, grant: bool, pick_best: bool) -> Option<&'static str> {
    let needs_grant = policy == "grant" || policy == "grant_or_pick";
    if grant && pick_best {
        Some(
            "give one more allowance with --grant or pick the best attempt with --pick-best, not both",
        )
    } else if pick_best && policy != "grant_or_pick" {
        Some(
            "this block is not a spent limit of a step whose gate judged its attempts, so there is nothing to pick: resume without --pick-best",
        )
    } else if pick_best {
        None
    } else if grant && !needs_grant {
        Some(
            "this block is not a spent limit, so there is nothing to grant: resume without --grant",
        )
    } else if !grant && policy == "grant_or_pick" {
        Some(
            "this run stopped at a spent limit: resume it with --grant to give one more round or the card's time or tokens again, or with --pick-best to take the best attempt its gate judged and go on",
        )
    } else if !grant && needs_grant {
        Some(
            "this run stopped at a spent limit: resume it with --grant to give one more round or the card's time or tokens again",
        )
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::resume_refusal;

    #[test]
    fn a_spent_limit_resumes_only_with_a_grant_or_a_pick_it_allows() {
        assert!(resume_refusal("grant", false, false).is_some());
        assert!(resume_refusal("grant", true, false).is_none());
        assert!(resume_refusal("grant", false, true).is_some());
        assert!(resume_refusal("grant_or_pick", false, true).is_none());
        assert!(resume_refusal("grant_or_pick", true, false).is_none());
        assert!(resume_refusal("grant_or_pick", false, false).is_some());
        assert!(resume_refusal("grant_or_pick", true, true).is_some());
        assert!(resume_refusal("explicit", true, false).is_some());
        assert!(resume_refusal("explicit", false, true).is_some());
        assert!(resume_refusal("explicit", false, false).is_none());
    }
}
