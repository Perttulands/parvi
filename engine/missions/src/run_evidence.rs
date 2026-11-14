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
        // What the run's agents wrote into its folder: a mission need not
        // declare a file for the operator to find it.
        let mut written = Vec::new();
        if only.is_none()
            && let Some(first) = events.first()
            && let Some(cwd) = first.data.get("cwd").and_then(Value::as_str)
            && let Ok(started) = time::OffsetDateTime::parse(
                &first.timestamp,
                &time::format_description::well_known::Rfc3339,
            )
        {
            // A finished run wrote nothing after it ended: what changed
            // later in its folder is another run's or the operator's.
            let ended = events
                .iter()
                .rev()
                .find(|e| {
                    matches!(
                        e.kind.as_str(),
                        "run_started"
                            | "run_resumed"
                            | "run_succeeded"
                            | "run_failed"
                            | "run_picked"
                            | "run_canceled"
                    )
                })
                .filter(|e| !matches!(e.kind.as_str(), "run_started" | "run_resumed"))
                .and_then(|e| {
                    time::OffsetDateTime::parse(
                        &e.timestamp,
                        &time::format_description::well_known::Rfc3339,
                    )
                    .ok()
                })
                .map(|at| std::time::SystemTime::from(at) + SETTLE);
            changed(Path::new(cwd), started.into(), ended, 0, &mut written);
        }
        Ok(
            json!({"runId":id,"problems":problems,"briefs":briefs,"files":files,"written":written,"truncated":truncated}),
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
/// How long after a run ends a file it wrote may still be landing.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

/// Files under `dir` changed since `since` (and before `until`, when the
/// run has ended), leaving out hidden ones (a checkout's .git among them),
/// a few levels deep and at most a page.
fn changed(
    dir: &Path,
    since: std::time::SystemTime,
    until: Option<std::time::SystemTime>,
    depth: usize,
    out: &mut Vec<Value>,
) {
    const MOST: usize = 200;
    if depth > 4 || out.len() >= MOST {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if out.len() >= MOST || entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            changed(&entry.path(), since, until, depth + 1, out);
        } else if kind.is_file()
            && let Ok(meta) = entry.metadata()
            && meta
                .modified()
                .is_ok_and(|m| m >= since && until.is_none_or(|end| m <= end))
        {
            out.push(json!({"path":entry.path(),"size":meta.len()}));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn lists_files_written_since_the_start_and_leaves_hidden_ones_out() {
        let dir = parvi_testkit::fresh_dir("written");
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("plan.txt"), "plan").unwrap();
        fs::write(dir.join("notes/review.txt"), "ok").unwrap();
        fs::write(dir.join(".git/HEAD"), "ref").unwrap();
        let started = SystemTime::now() - Duration::from_secs(60);
        let mut written = Vec::new();
        changed(&dir, started, None, 0, &mut written);
        let paths: Vec<_> = written
            .iter()
            .map(|w| w["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                dir.join("notes/review.txt").to_str().unwrap(),
                dir.join("plan.txt").to_str().unwrap()
            ]
        );
        let mut later = Vec::new();
        changed(
            &dir,
            SystemTime::now() + Duration::from_secs(60),
            None,
            0,
            &mut later,
        );
        assert!(later.is_empty());
        // A run that ended before the files were written did not write them.
        let mut ended = Vec::new();
        changed(
            &dir,
            started,
            Some(SystemTime::now() - Duration::from_secs(30)),
            0,
            &mut ended,
        );
        assert!(ended.is_empty());
    }
}
