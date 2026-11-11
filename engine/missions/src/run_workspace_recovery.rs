//! Recover pending turn publications and clean abandoned worktrees.
use crate::coordinator::Control;
use crate::run_worker::append_live;
use crate::run_workspace::{commit_turn, remove, turn_path};
use crate::{Coordinator, RunEvent, RunStart};
use std::{io, path::Path};

/// Retire unaccepted attempts after executors have stopped, preserving pending
/// outputs for publication on resume. Also retries cleanup after a crash.
pub(crate) fn cleanup(
    c: &Coordinator,
    id: &str,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
) -> io::Result<()> {
    if req.repository.is_empty() {
        return Ok(());
    }
    let _guard = control.workspace.lock().unwrap();
    let events = c.ledger(id)?.read()?;
    for start in events
        .iter()
        .filter(|e| e.kind == "workspace_started" || e.kind == "workspace_settled")
    {
        let same = |e: &&RunEvent| e.node_id == start.node_id && e.attempt == start.attempt;
        let pending = events.iter().filter(same).any(|e| {
            e.kind == "node_output_pending"
                || (e.kind == "output_capture_pending"
                    && !crate::run_capture::discarded(&events, e))
        });
        let settled = events
            .iter()
            .filter(same)
            .any(|e| e.kind == "workspace_settled");
        if !pending || settled || control.canceled.load(std::sync::atomic::Ordering::Acquire) {
            remove(workspace, &turn_path(c, id, &start.node_id, start.attempt))?;
        }
    }
    Ok(())
}

pub(crate) fn recover_pending(
    c: &Coordinator,
    id: &str,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
) -> io::Result<()> {
    if req.repository.is_empty() {
        return Ok(());
    }
    cleanup(c, id, req, workspace, control)?;
    let events = c.ledger(id)?.read()?;
    for pending in events.iter().filter(|e| e.kind == "node_output_pending") {
        if events.iter().any(|e| {
            e.kind == "node_output" && e.node_id == pending.node_id && e.attempt == pending.attempt
        }) {
            continue;
        }
        commit_turn(
            c,
            id,
            req,
            workspace,
            control,
            &pending.node_id,
            pending.attempt,
        )?;
        let mut output = pending.clone();
        output.kind = "node_output".into();
        output.seq = 0;
        if !append_live(c, id, control, output)? {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
        }
    }
    Ok(())
}
