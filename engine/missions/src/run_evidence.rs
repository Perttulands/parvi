//! Paths and problems recorded by one run, without reading artifact contents.
use crate::{Coordinator, RunEvent};
use parvi_mission_model::EvidenceText;
use serde_json::{Value, json};
use std::{fs, io, path::Path};

impl Coordinator {
    pub fn evidence(&self, id: &str, only: Option<&str>) -> Result<Value, crate::Error> {
        let events = self
            .ledger(id)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::InvalidInput {
                    crate::Error::new("unknown_run", format!("run {id:?} not found"))
                } else {
                    crate::error::io(error)
                }
            })?
            .read()
            .map_err(|error| {
                if error.code == "not_found" || error.code == "invalid_input" {
                    crate::Error::new("unknown_run", format!("run {id:?} not found"))
                } else {
                    error
                }
            })?;
        let mission = self.frozen_mission(id)?;
        if let Some(step) = only
            && mission.formation(step).is_none()
        {
            return Err(crate::Error::new(
                "not_found",
                format!("step {step:?} not found"),
            ));
        }
        let mut problems = Vec::new();
        let mut briefs = Vec::new();
        for event in &events {
            let origin = event
                .data
                .get("inputRef")
                .and_then(|v| v.get("origin"))
                .and_then(|v| v.get("nodeId"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let blocked = event
                .data
                .get("blockedNodeId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let related = only.is_none_or(|step| {
                event.node_id == step
                    || origin == step
                    || blocked == step
                    || event
                        .data
                        .get("openDispatches")
                        .and_then(Value::as_array)
                        .is_some_and(|ds| {
                            ds.iter()
                                .any(|d| d.get("nodeId").and_then(Value::as_str) == Some(step))
                        })
            });
            if !related {
                continue;
            }
            if event.kind == "slot_dispatch" {
                if let Some(path) = event
                    .data
                    .get("briefPath")
                    .and_then(Value::as_str)
                    .filter(|p| Path::new(p).is_absolute())
                {
                    briefs.push(json!({"nodeId":event.node_id,"slotId":event.slot_id,"attempt":event.attempt,"dispatchSeq":event.seq,"path":path}));
                }
            } else if problem(event) {
                let reason = ["reason", "detail", "message"]
                    .iter()
                    .find_map(|key| {
                        event
                            .data
                            .get(*key)
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                    })
                    .unwrap_or("");
                problems.push(json!({"seq":event.seq,"type":event.kind,"nodeId":if !origin.is_empty() {origin} else if !event.node_id.is_empty() {&event.node_id} else {blocked},"gateId":event.gate_id,"code":event.data.get("code"),"reason":EvidenceText::of(reason)}));
            }
        }
        let mut files = Vec::new();
        let mut truncated = false;
        let root = self.inner.root.join("artifacts").join(id);
        for formation in &mission.formations {
            if only.is_some_and(|step| step != formation.id) {
                continue;
            }
            let dir = root.join(&formation.id);
            walk(&dir, &formation.id, 0, &mut files, &mut truncated).map_err(crate::error::io)?;
        }
        Ok(
            json!({"runId":id,"problems":problems,"briefs":briefs,"files":files,"truncated":truncated}),
        )
    }
}
fn problem(event: &RunEvent) -> bool {
    match event.kind.as_str() {
        "run_blocked" | "run_failed" | "error" | "judge_attempt_failed" | "seat_failed" => true,
        "gate_verdict" | "gate_kind_result" => {
            event.data.get("verdict").and_then(Value::as_str) == Some("fail")
        }
        "slot_result" => event
            .data
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|s| s != "ok"),
        _ => false,
    }
}
fn walk(
    dir: &Path,
    node: &str,
    depth: usize,
    files: &mut Vec<Value>,
    truncated: &mut bool,
) -> io::Result<()> {
    if depth > 16 {
        *truncated = true;
        return Ok(());
    }
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => return Ok(()),
        Ok(_) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut entries = entries.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if files.len() >= 500 {
            *truncated = true;
            break;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".capture-") || name == ".outputs-backup" {
            continue;
        }
        if entry.file_type()?.is_symlink() {
            continue;
        }
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            walk(&entry.path(), node, depth + 1, files, truncated)?;
        } else if metadata.is_file() {
            let path = entry.path().canonicalize()?;
            files.push(json!({"nodeId":node,"path":path,"size":metadata.len()}));
        }
    }
    Ok(())
}
