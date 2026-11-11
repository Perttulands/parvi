//! Capture a finished attempt before routing it, and retry a pending capture.
use crate::{
    Coordinator, Mission, RunEvent, RunStart, coordinator::Control, run_worker::append_live,
};
use serde_json::json;
use std::{
    collections::HashSet,
    fs, io,
    path::{Component, Path, PathBuf},
};

pub(crate) fn attempt_dir(
    c: &Coordinator,
    id: &str,
    node: &str,
    attempt: i64,
) -> io::Result<PathBuf> {
    fn component(s: &str) -> bool {
        let mut c = Path::new(s).components();
        matches!(c.next(), Some(Component::Normal(_))) && c.next().is_none()
    }
    if !component(id) || !component(node) || attempt < 1 {
        return Err(io::Error::other("invalid artifact identity"));
    }
    Ok(c.inner
        .root
        .join("artifacts")
        .join(id)
        .join(node)
        .join(attempt.to_string()))
}

pub(crate) fn publish(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
    output: RunEvent,
) -> io::Result<()> {
    let Some(formation) = mission.formation(&output.node_id) else {
        return crate::run_workspace::publish_output(c, id, req, workspace, control, output);
    };
    if !formation.outputs.iter().any(|p| p.kind() != "text") {
        return crate::run_workspace::publish_output(c, id, req, workspace, control, output);
    }
    validate_refs(formation, &output)?;
    let mut pending = output.clone();
    pending.kind = "output_capture_pending".into();
    if !append_live(c, id, control, pending)? {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
    }
    accept(c, id, mission, req, workspace, control, output)
}

fn validate_refs(formation: &crate::Formation, output: &RunEvent) -> io::Result<()> {
    let payloads = output
        .data
        .get("outputs")
        .and_then(|v| v.as_object())
        .ok_or_else(|| io::Error::other("invalid_output_payloads"))?;
    for port in formation.outputs.iter().filter(|p| p.kind() != "text") {
        let value = payloads
            .get(&port.id)
            .and_then(|v| v.get("ref"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let path = Path::new(value);
        if !path.is_absolute() || value.contains("://") || value.contains('\0') {
            return Err(io::Error::other(format!(
                "invalid_output_ref: {} output {} requires an absolute path",
                output.node_id, port.id
            )));
        }
        let meta = fs::metadata(path)
            .map_err(|e| io::Error::other(format!("unavailable_output_ref: {value}: {e}")))?;
        if (port.kind() == "file" && !meta.is_file()) || (port.kind() == "folder" && !meta.is_dir())
        {
            return Err(io::Error::other(format!(
                "invalid_output_ref: {value} is not a {}",
                port.kind()
            )));
        }
        if Path::new(&port.id).components().count() != 1
            || !matches!(
                Path::new(&port.id).components().next(),
                Some(Component::Normal(_))
            )
        {
            return Err(io::Error::other("invalid_output_ref: invalid port id"));
        }
    }
    Ok(())
}

fn accept(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
    mut output: RunEvent,
) -> io::Result<()> {
    let formation = mission
        .formation(&output.node_id)
        .ok_or_else(|| io::Error::other("missing formation"))?;
    validate_refs(formation, &output)?;
    let attempt = attempt_dir(c, id, &output.node_id, output.attempt)?;
    fs::create_dir_all(&attempt).map_err(|e| io::Error::other(format!("output_not_kept: {e}")))?;
    let target = attempt.join("outputs");
    if let Ok(meta) = fs::symlink_metadata(&target)
        && !meta.is_dir()
    {
        return Err(io::Error::other(
            "output_not_kept: outputs is not a directory",
        ));
    }
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|e| io::Error::other(format!("output_not_kept: {e}")))?;
    let stage = attempt.join(format!(".capture-{:x}", u64::from_ne_bytes(random)));
    let mut prepared = output.clone();
    prepared.kind = "output_capture_prepared".into();
    prepared.data = json!({"stage":stage.file_name().unwrap().to_string_lossy()})
        .as_object()
        .unwrap()
        .clone();
    if !append_live(c, id, control, prepared)? {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
    }
    fs::create_dir(&stage).map_err(|e| io::Error::other(format!("output_not_kept: {e}")))?;
    let mut budget = crate::Budget {
        remaining: crate::kept_copy::CAPTURE_BYTES.saturating_sub(used(
            c,
            id,
            &output.node_id,
            output.attempt,
        )?),
    };
    let result = (|| {
        let payloads = output
            .data
            .get_mut("outputs")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| io::Error::other("invalid_output_payloads"))?;
        for port in &formation.outputs {
            let kind = port.kind();
            if kind == "text" {
                continue;
            }
            let payload = payloads
                .get_mut(&port.id)
                .and_then(|v| v.as_object_mut())
                .ok_or_else(|| io::Error::other("missing_output_payload"))?;
            let value = payload
                .get("ref")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let source = Path::new(value);
            let kept = crate::KeptCopy::keep_cancelable(
                source,
                kind,
                &stage.join(&port.id),
                &mut budget,
                &control.canceled,
                &[c.inner.root.join("artifacts").join(id), stage.clone()],
            )
            .map_err(|e| io::Error::other(format!("output_not_kept: {}", e.0)))?;
            let name = kept
                .file_name()
                .ok_or_else(|| io::Error::other("output_not_kept: missing name"))?;
            payload.insert("ref".into(), json!(target.join(&port.id).join(name)));
            payload.insert("kind".into(), json!(kind));
        }
        let backup = attempt.join(".outputs-backup");
        if backup.exists() {
            return Err(io::Error::other(
                "output_not_kept: pending output backup requires recovery",
            ));
        }
        if target.exists() {
            fs::rename(&target, &backup)
                .map_err(|e| io::Error::other(format!("output_not_kept: {e}")))?;
        }
        fs::rename(&stage, &target)
            .map_err(|e| io::Error::other(format!("output_not_kept: {e}")))?;
        crate::kept_copy::freeze_tree(&target)
            .map_err(|e| io::Error::other(format!("output_not_kept: {e}")))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = restore(&attempt, &stage);
    }
    result?;
    crate::run_workspace::publish_output(c, id, req, workspace, control, output)?;
    crate::kept_copy::remove_tree(&attempt.join(".outputs-backup"))?;
    Ok(())
}

fn restore(attempt: &Path, stage: &Path) -> io::Result<()> {
    let backup = attempt.join(".outputs-backup");
    let target = attempt.join("outputs");
    if backup.exists() {
        crate::kept_copy::remove_tree(&target)?;
        fs::rename(backup, target)?;
    }
    crate::kept_copy::remove_tree(stage)
}

fn size(path: &Path) -> io::Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(0);
    }
    if meta.is_file() {
        return Ok(meta.len());
    }
    if !meta.is_dir() {
        return Ok(0);
    }
    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        total = total.saturating_add(size(&entry?.path())?);
    }
    Ok(total)
}
fn used(c: &Coordinator, id: &str, node: &str, attempt: i64) -> io::Result<u64> {
    let events = c.ledger(id)?.read().map_err(crate::error::to_io)?;
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for event in events
        .iter()
        .filter(|e| e.kind == "node_output" && !(e.node_id == node && e.attempt == attempt))
    {
        for payload in event
            .data
            .get("outputs")
            .and_then(|v| v.as_object())
            .into_iter()
            .flat_map(|m| m.values())
        {
            if !matches!(
                payload.get("kind").and_then(|v| v.as_str()),
                Some("file" | "folder")
            ) {
                continue;
            }
            if let Some(path) = payload.get("ref").and_then(|v| v.as_str())
                && seen.insert(path.to_owned())
            {
                total = total.saturating_add(size(Path::new(path))?);
            }
        }
    }
    Ok(total)
}

pub(crate) fn recover(
    c: &Coordinator,
    id: &str,
    mission: &Mission,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
) -> io::Result<()> {
    let events = c.ledger(id)?.read().map_err(crate::error::to_io)?;
    for pending in events.iter().filter(|e| e.kind == "output_capture_pending") {
        let attempt = attempt_dir(c, id, &pending.node_id, pending.attempt)?;
        let accepted = events.iter().any(|e| {
            (e.kind == "node_output" || e.kind == "node_output_pending")
                && e.node_id == pending.node_id
                && e.attempt == pending.attempt
        });
        for prepared in events.iter().filter(|e| {
            e.kind == "output_capture_prepared"
                && e.node_id == pending.node_id
                && e.attempt == pending.attempt
                && e.seq > pending.seq
        }) {
            let name = prepared
                .data
                .get("stage")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !name.starts_with(".capture-")
                || !matches!(
                    Path::new(name).components().next(),
                    Some(Component::Normal(_))
                )
                || Path::new(name).components().count() != 1
            {
                return Err(io::Error::other("invalid capture staging path"));
            }
            if accepted {
                crate::kept_copy::remove_tree(&attempt.join(name))?;
            } else {
                restore(&attempt, &attempt.join(name))?;
            }
        }
        if accepted {
            crate::kept_copy::remove_tree(&attempt.join(".outputs-backup"))?;
        }
        if discarded(&events, pending)
            || events.iter().any(|e| {
                (e.kind == "node_output" || e.kind == "node_output_pending")
                    && e.node_id == pending.node_id
                    && e.attempt == pending.attempt
            })
        {
            continue;
        }
        let mut output = pending.clone();
        output.kind = "node_output".into();
        output.seq = 0;
        accept(c, id, mission, req, workspace, control, output)?;
    }
    Ok(())
}

pub(crate) fn discarded(events: &[RunEvent], pending: &RunEvent) -> bool {
    events.iter().any(|event| {
        event.seq > pending.seq
            && event.kind == "run_resumed"
            && event.data.get("resumeMode").and_then(|v| v.as_str()) == Some("redispatch")
            && event.data.get("redispatchNodeId").and_then(|v| v.as_str()) == Some(&pending.node_id)
            && event.data.get("redispatchAttempt").and_then(|v| v.as_i64()) == Some(pending.attempt)
    })
}
