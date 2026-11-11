//! Durable, validated run event ledgers.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
const RUN_EVENT_MAX_BYTES: usize = 1 << 20;

/// One durable event. Event data is private until explicitly projected.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEvent {
    #[serde(rename = "ts")]
    pub timestamp: String,
    pub run_id: String,
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub actor: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mission_id: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub mission_rev: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub input_card_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub work: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slot_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gate_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub edge_id: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub epoch: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attempt: i64,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub data: Map<String, Value>,
}
fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// A ledger's path and identity. All operations lock the file itself, so
/// independent handles cannot interleave reads and appends.
#[derive(Clone, Debug)]
pub struct Ledger {
    path: PathBuf,
    run_id: String,
}

impl Ledger {
    pub fn new(path: impl Into<PathBuf>, run_id: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            run_id: run_id.into(),
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Atomically create the ledger and sync its first event before returning.
    pub fn create(&self, mut event: RunEvent) -> io::Result<()> {
        if self.run_id.is_empty() || event.kind != "run_started" {
            return Err(invalid("first event must be run_started"));
        }
        event.run_id.clone_from(&self.run_id);
        event.seq = 1;
        validate(std::slice::from_ref(&event), &self.run_id)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.path)?;
        let result = write_event(&mut file, &event).and_then(|()| sync_parent(&self.path));
        if result.is_err() {
            drop(file);
            let _ = std::fs::remove_file(&self.path);
        }
        result
    }

    pub fn read(&self) -> io::Result<Vec<RunEvent>> {
        let mut file = OpenOptions::new().read(true).open(&self.path)?;
        file.lock_shared()?;
        read_file(&mut file, &self.run_id)
    }

    /// Assign the next sequence while holding the lock, and sync before return.
    pub fn append(&self, event: RunEvent) -> io::Result<RunEvent> {
        self.append_checked(event, |_| Ok(()))
    }

    /// Validate against the locked ledger before assigning the next sequence.
    pub(crate) fn append_checked(
        &self,
        mut event: RunEvent,
        check: impl FnOnce(&[RunEvent]) -> io::Result<()>,
    ) -> io::Result<RunEvent> {
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.path)?;
        file.lock()?;
        let events = read_file(&mut file, &self.run_id)?;
        check(&events)?;
        let first = &events[0];
        let last = events.last().expect("validated ledger is nonempty");
        if matches!(
            last.kind.as_str(),
            "run_succeeded" | "run_failed" | "run_canceled" | "run_picked"
        ) {
            return Err(invalid("run is final"));
        }
        let lifecycle = events.iter().rev().find(|entry| {
            matches!(
                entry.kind.as_str(),
                "run_started"
                    | "run_resumed"
                    | "run_blocked"
                    | "run_succeeded"
                    | "run_failed"
                    | "run_canceled"
                    | "run_picked"
            )
        });
        if let Some(last_lifecycle) = lifecycle {
            if last_lifecycle.kind == "run_blocked" {
                if !matches!(
                    event.kind.as_str(),
                    "run_resumed"
                        | "run_canceled"
                        | "run_failed"
                        | "seat_cleanup"
                        | "human_verdict_recorded"
                ) {
                    return Err(invalid("run epoch is blocked"));
                }
                if event.kind == "run_resumed" {
                    if !last_lifecycle
                        .data
                        .get("resumeAllowed")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        return Err(invalid("run resume is not allowed"));
                    }
                    if event.epoch == 0 {
                        event.epoch = last_lifecycle.epoch + 1;
                    }
                }
            } else if event.kind == "run_resumed" {
                return Err(invalid("run resume is not allowed"));
            }
        }
        event.run_id.clone_from(&self.run_id);
        event.seq = last.seq + 1;
        if event.actor.is_empty() {
            event.actor.clone_from(&first.actor);
        }
        if event.mission_id.is_empty() {
            event.mission_id.clone_from(&first.mission_id);
        }
        if event.mission_rev == 0 {
            event.mission_rev = first.mission_rev;
        }
        if event.input_card_id.is_empty() {
            event.input_card_id.clone_from(&first.input_card_id);
        }
        if event.work.is_empty() {
            event.work.clone_from(&first.work);
        }
        if event.epoch == 0 {
            event.epoch = last.epoch;
        }
        validate_event(&event, &self.run_id, event.seq)?;
        write_event(&mut file, &event)?;
        Ok(event)
    }
}

pub fn validate(events: &[RunEvent], run_id: &str) -> io::Result<()> {
    if events
        .first()
        .is_none_or(|event| event.kind != "run_started")
    {
        return Err(invalid("first event must be run_started"));
    }
    for (i, event) in events.iter().enumerate() {
        validate_event(event, run_id, i as u64 + 1)?;
    }
    Ok(())
}
fn validate_event(event: &RunEvent, run_id: &str, seq: u64) -> io::Result<()> {
    if event.run_id != run_id
        || run_id.is_empty()
        || event.seq != seq
        || event.kind.is_empty()
        || event.actor.is_empty()
        || event.timestamp.is_empty()
    {
        return Err(invalid(
            "event identity, sequence, timestamp, type and actor are required",
        ));
    }
    Ok(())
}
fn read_file(file: &mut File, run_id: &str) -> io::Result<Vec<RunEvent>> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if !bytes.ends_with(b"\n") {
        return Err(invalid("incomplete ledger line"));
    }
    let mut events = Vec::new();
    for line in bytes[..bytes.len() - 1].split(|byte| *byte == b'\n') {
        if line.len() > RUN_EVENT_MAX_BYTES {
            return Err(invalid("run event exceeds byte limit"));
        }
        events.push(serde_json::from_slice(line).map_err(|err| invalid(&err.to_string()))?);
    }
    validate(&events, run_id)?;
    Ok(events)
}
fn write_event(file: &mut File, event: &RunEvent) -> io::Result<()> {
    let mut line = serde_json::to_vec(event)?;
    if line.len() > RUN_EVENT_MAX_BYTES {
        return Err(invalid("run event exceeds byte limit"));
    }
    line.push(b'\n');
    file.write_all(&line)?;
    file.sync_all()
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> io::Result<()> {
    File::open(path.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()
}
#[cfg(not(unix))]
fn sync_parent(_: &Path) -> io::Result<()> {
    Ok(())
}
