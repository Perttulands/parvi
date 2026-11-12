//! Durable records for dispatches owned by this node.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    pub id: String,
    pub run_id: String,
    pub node_id: String,
    pub slot_id: String,
    /// The readable stem of the seat's session name; blank names it by ids.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub harness: String,
    pub model: String,
    pub effort: String,
    pub cwd: PathBuf,
    pub brief: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reuse_dispatch: Option<String>,
    /// The environment the seat runs on, `<device>/<environment>`; blank is
    /// the coordinator's own.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub place: String,
    /// The turn worktree a placed seat works in, kept so a coordinator that
    /// restarts can still publish the turn once the seat has ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<crate::TurnWorkspace>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatEvent {
    /// Unique within a dispatch; retries with the same key must agree.
    pub key: String,
    pub seq: u64,
    pub kind: String,
    pub data: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchResult {
    pub text: String,
    pub status: String,
}

/// The immutable backend identity retained for cleanup and reattachment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatId {
    pub node: String,
    pub session_name: String,
    pub session: parvi_term::SessionRef,
}

/// Public location and state of one seat; server-generation proof stays private.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSeat {
    pub dispatch_id: String,
    pub slot_id: String,
    pub node: String,
    pub session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane: Option<parvi_term::control::PaneId>,
    pub session_name: String,
    pub outcome: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting: Option<SeatWait>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatWait {
    pub state: String,
    pub detail: String,
    pub since: String,
    pub seq: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    dispatch: Dispatch,
    #[serde(default)]
    seat: Option<SeatId>,
    events: Vec<SeatEvent>,
    result: Option<DispatchResult>,
}

/// File-backed dispatch records. Independent instances serialize through a
/// per-dispatch file lock; published records are replaced atomically.
#[derive(Clone, Debug)]
pub struct SeatRecords {
    root: PathBuf,
}
impl SeatRecords {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir().unwrap_or_default().join(root)
        };
        Self {
            root: root.join("seats"),
        }
    }

    pub fn create(&self, dispatch: &Dispatch) -> Result<(), crate::Error> {
        (|| -> io::Result<()> {
            let dir = self.directory(&dispatch.id)?;
            self.locked(&dir, true, || {
                let path = dir.join("dispatch.json");
                match read(&path) {
                    Ok(existing) => {
                        let existing: Record = existing;
                        if existing.dispatch == *dispatch {
                            Ok(())
                        } else {
                            Err(conflict("dispatch id already belongs to another dispatch"))
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        publish(&dir.join("brief.md"), dispatch.brief.as_bytes())?;
                        write_record(
                            &path,
                            &Record {
                                dispatch: dispatch.clone(),
                                seat: None,
                                events: Vec::new(),
                                result: None,
                            },
                        )
                    }
                    Err(e) => Err(e),
                }
            })
        })()
        .map_err(crate::error::io)
    }

    pub fn dispatch(&self, id: &str) -> Result<Dispatch, crate::Error> {
        self.with_record(id, |record| Ok(record.dispatch.clone()))
            .map_err(crate::error::io)
    }
    pub fn brief_path(&self, id: &str) -> Result<PathBuf, crate::Error> {
        self.with_record(id, |_| Ok(self.directory(id)?.join("brief.md")))
            .map_err(crate::error::io)
    }
    pub fn events(&self, id: &str, since: u64) -> Result<Vec<SeatEvent>, crate::Error> {
        self.with_record(id, |record| {
            Ok(record
                .events
                .iter()
                .filter(|event| event.seq > since)
                .cloned()
                .collect())
        })
        .map_err(crate::error::io)
    }
    pub fn result(&self, id: &str) -> Result<Option<DispatchResult>, crate::Error> {
        self.with_record(id, |record| Ok(record.result.clone()))
            .map_err(crate::error::io)
    }
    pub fn seat(&self, id: &str) -> Result<Option<SeatId>, crate::Error> {
        self.with_record(id, |record| Ok(record.seat.clone()))
            .map_err(crate::error::io)
    }
    pub(super) fn dispatch_for_seat(&self, seat: &SeatId) -> io::Result<String> {
        let mut matches = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            match self.with_record(&id, |record| {
                Ok((
                    record.seat.as_ref() == Some(seat),
                    record.dispatch.reuse_dispatch.is_none(),
                ))
            }) {
                Ok((true, original)) => matches.push((!original, id)),
                Ok(_) => (),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        matches.sort();
        matches
            .into_iter()
            .next()
            .map(|(_, id)| id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown seat"))
    }
    pub fn seats(&self, run_id: &str) -> Result<Vec<AgentSeat>, crate::Error> {
        (|| -> io::Result<Vec<AgentSeat>> {
            let entries = match fs::read_dir(&self.root) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(error) => return Err(error),
            };
            let mut seats = Vec::new();
            for entry in entries {
                let entry = entry?;
                let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let result = self.with_record(&id, |record| {
                    if record.dispatch.run_id != run_id {
                        return Ok(None);
                    }
                    let Some(seat) = &record.seat else {
                        return Ok(None);
                    };
                    let outcome = record
                        .events
                        .iter()
                        .rev()
                        .find(|event| event.kind == "seat_cleanup")
                        .and_then(|event| event.data.get("outcome"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let waiting = record
                        .events
                        .iter()
                        .rev()
                        .find(|event| {
                            matches!(
                                event.kind.as_str(),
                                "seat_created"
                                    | "seat_prompt_consumed"
                                    | "slot_result"
                                    | "seat_cleanup"
                                    | "seat_state"
                                    | "seat_idle_recovery"
                            )
                        })
                        .filter(|event| {
                            event.kind == "seat_state"
                                && event.data.get("state").and_then(Value::as_str)
                                    != Some("working")
                        })
                        .map(|event| SeatWait {
                            state: event
                                .data
                                .get("state")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .into(),
                            detail: event
                                .data
                                .get("detail")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .into(),
                            since: event
                                .data
                                .get("since")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .into(),
                            seq: event.seq,
                        });
                    Ok(Some(AgentSeat {
                        dispatch_id: record.dispatch.id.clone(),
                        slot_id: record.dispatch.slot_id.clone(),
                        node: seat.node.clone(),
                        session: seat.session.session_id.clone(),
                        pane: Some(seat.session.pane_id),
                        session_name: seat.session_name.clone(),
                        outcome: outcome.into(),
                        available: outcome.is_empty() || outcome == "left_shutdown",
                        waiting,
                    }))
                });
                match result {
                    Ok(Some(seat)) => seats.push(seat),
                    Ok(None) => (),
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
                        ) => {}
                    Err(error) => return Err(error),
                }
            }
            seats.sort_by(|a, b| a.dispatch_id.cmp(&b.dispatch_id));
            Ok(seats)
        })()
        .map_err(crate::error::io)
    }
    /// Remove every dispatch folder (brief, record, locks) of a deleted run.
    /// A folder whose record cannot be read is not this run's to remove.
    pub fn forget_run(&self, run_id: &str) -> Result<(), crate::Error> {
        (|| -> io::Result<()> {
            let entries = match fs::read_dir(&self.root) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            for entry in entries {
                let entry = entry?;
                let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if self
                    .with_record(&id, |record| Ok(record.dispatch.run_id == run_id))
                    .unwrap_or(false)
                {
                    crate::kept_copy::remove_tree(&entry.path())?;
                }
            }
            Ok(())
        })()
        .map_err(crate::error::io)
    }
    pub fn bind_seat(&self, id: &str, seat: SeatId) -> Result<(), crate::Error> {
        self.change(id, |record| match &record.seat {
            Some(existing) if *existing == seat => Ok(((), false)),
            Some(_) => Err(conflict("dispatch already has a different seat")),
            None => {
                record.seat = Some(seat);
                Ok(((), true))
            }
        })
        .map_err(crate::error::io)
    }
    pub fn append(
        &self,
        id: &str,
        key: &str,
        kind: &str,
        data: Map<String, Value>,
    ) -> Result<SeatEvent, crate::Error> {
        (|| -> io::Result<SeatEvent> {
            if key.is_empty() || kind.is_empty() {
                return Err(invalid("event key and kind are required"));
            }
            self.change(id, |record| {
                if let Some(event) = record.events.iter().find(|event| event.key == key) {
                    return if event.kind == kind && event.data == data {
                        Ok((event.clone(), false))
                    } else {
                        Err(conflict("event key already used"))
                    };
                }
                let event = SeatEvent {
                    key: key.into(),
                    seq: record.events.len() as u64 + 1,
                    kind: kind.into(),
                    data,
                };
                record.events.push(event.clone());
                Ok((event, true))
            })
        })()
        .map_err(crate::error::io)
    }
    pub fn finish(&self, id: &str, result: DispatchResult) -> Result<(), crate::Error> {
        self.change(id, |record| match &record.result {
            Some(existing) if *existing == result => Ok(((), false)),
            Some(_) => Err(conflict("dispatch already has a different result")),
            None => {
                record.result = Some(result);
                Ok(((), true))
            }
        })
        .map_err(crate::error::io)
    }
    fn directory(&self, id: &str) -> io::Result<PathBuf> {
        if id.is_empty()
            || id.len() > 200
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(invalid("invalid dispatch id"));
        }
        Ok(self.root.join(id))
    }
    pub(super) fn creation_guard(&self, id: &str) -> io::Result<fs::File> {
        self.operation_guard(id, "creation.lock")
    }
    pub(super) fn execution_guard(&self, id: &str) -> io::Result<fs::File> {
        self.operation_guard(id, "execution.lock")
    }
    fn operation_guard(&self, id: &str, name: &str) -> io::Result<fs::File> {
        let dir = self.directory(id)?;
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(dir.join(name))?;
        file.lock()?;
        Ok(file)
    }
    fn with_record<T>(&self, id: &str, f: impl FnOnce(&Record) -> io::Result<T>) -> io::Result<T> {
        let dir = self.directory(id)?;
        self.locked(&dir, false, || f(&read(&dir.join("dispatch.json"))?))
    }
    fn change<T>(
        &self,
        id: &str,
        f: impl FnOnce(&mut Record) -> io::Result<(T, bool)>,
    ) -> io::Result<T> {
        let dir = self.directory(id)?;
        self.locked(&dir, false, || {
            let path = dir.join("dispatch.json");
            let mut record = read(&path)?;
            let (value, changed) = f(&mut record)?;
            if changed {
                write_record(&path, &record)?;
            }
            Ok(value)
        })
    }
    fn locked<T>(
        &self,
        dir: &Path,
        create: bool,
        f: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<T> {
        if create {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(dir)?;
        }
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(create);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let lock = opts.open(dir.join("lock"))?;
        lock.lock()?;
        f()
    }
}
fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
fn write_record(path: &Path, record: &Record) -> io::Result<()> {
    publish(path, &serde_json::to_vec(record)?)
}
fn publish(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| invalid("missing parent"))?;
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let (temp, mut file) = loop {
        let temp = parent.join(format!(
            ".{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        match opts.open(&temp) {
            Ok(file) => break (temp, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn conflict(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn scratch() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "parvi-105-seat-records-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn request() -> Dispatch {
        Dispatch {
            id: "dsp_test".into(),
            run_id: "run_1".into(),
            node_id: "step".into(),
            slot_id: "worker".into(),
            name: String::new(),
            harness: "pi".into(),
            model: "model".into(),
            effort: "medium".into(),
            cwd: "/tmp".into(),
            brief: "private brief".into(),
            reuse_dispatch: None,
            place: String::new(),
            turn: None,
        }
    }
    #[test]
    fn forgetting_a_run_removes_only_its_dispatch_folders() {
        let dir = scratch();
        let store = SeatRecords::new(&dir);
        let mine = request();
        let other = Dispatch {
            id: "dsp_other".into(),
            run_id: "run_2".into(),
            ..request()
        };
        store.create(&mine).unwrap();
        store.create(&other).unwrap();
        let brief = store.brief_path(&mine.id).unwrap();
        crate::kept_copy::readonly(&brief).unwrap();
        store.forget_run("run_1").unwrap();
        assert!(!dir.join("seats").join("dsp_test").exists());
        assert!(store.dispatch("dsp_other").is_ok());
        store.forget_run("run_1").unwrap();
        crate::kept_copy::remove_tree(&dir).unwrap();
    }
    #[test]
    fn records_survive_reopen_and_retries_are_idempotent() {
        let dir = scratch();
        let store = SeatRecords::new(&dir);
        let req = request();
        store.create(&req).unwrap();
        store.create(&req).unwrap();
        let seat = SeatId {
            node: "local".into(),
            session_name: "parvi-run-worker".into(),
            session: parvi_term::SessionRef {
                session_id: "$1".into(),
                pane_id: 2,
                generation: parvi_term::Generation {
                    pid: 3,
                    start_time: 4,
                },
            },
        };
        store.bind_seat(&req.id, seat.clone()).unwrap();
        store.bind_seat(&req.id, seat.clone()).unwrap();
        let mut different = seat.clone();
        different.session.pane_id += 1;
        assert!(store.bind_seat(&req.id, different).is_err());
        let event = store
            .append(&req.id, "created", "seat_created", Map::new())
            .unwrap();
        assert_eq!(
            store
                .append(&req.id, "created", "seat_created", Map::new())
                .unwrap(),
            event
        );
        assert!(
            store
                .append(&req.id, "created", "other", Map::new())
                .is_err()
        );
        let result = DispatchResult {
            text: "done".into(),
            status: "ok".into(),
        };
        store.finish(&req.id, result.clone()).unwrap();
        store.finish(&req.id, result.clone()).unwrap();
        let reopened = SeatRecords::new(&dir);
        assert_eq!(reopened.dispatch(&req.id).unwrap(), req);
        assert_eq!(reopened.seat(&req.id).unwrap(), Some(seat));
        let seats = reopened.seats("run_1").unwrap();
        assert_eq!(seats.len(), 1);
        assert_eq!(
            (&seats[0].node, &seats[0].session, seats[0].pane),
            (&"local".into(), &"$1".into(), Some(2))
        );
        assert!(reopened.seats("run_other").unwrap().is_empty());
        assert_eq!(reopened.events(&req.id, 0).unwrap(), vec![event]);
        assert!(reopened.events(&req.id, 1).unwrap().is_empty());
        let data = serde_json::json!({"state":"brief_not_taken","detail":"waiting","since":"2026-11-06T00:00:00Z"});
        reopened
            .append(
                &req.id,
                "wait",
                "seat_state",
                data.as_object().unwrap().clone(),
            )
            .unwrap();
        assert_eq!(
            reopened.seats("run_1").unwrap()[0]
                .waiting
                .as_ref()
                .unwrap()
                .state,
            "brief_not_taken"
        );
        let data = serde_json::json!({"state":"working"});
        reopened
            .append(
                &req.id,
                "resumed",
                "seat_state",
                data.as_object().unwrap().clone(),
            )
            .unwrap();
        assert!(reopened.seats("run_1").unwrap()[0].waiting.is_none());
        assert_eq!(reopened.result(&req.id).unwrap(), Some(result));
        assert_eq!(
            fs::read_to_string(reopened.brief_path(&req.id).unwrap()).unwrap(),
            "private brief"
        );
        assert!(reopened.dispatch("../escape").is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn concurrent_handles_assign_distinct_sequences() {
        let dir = scratch();
        let store = Arc::new(SeatRecords::new(&dir));
        store.create(&request()).unwrap();
        let threads: Vec<_> = (0..16)
            .map(|i| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store
                        .append("dsp_test", &format!("event-{i}"), "state", Map::new())
                        .unwrap()
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let events = store.events("dsp_test", 0).unwrap();
        assert_eq!(
            events.iter().map(|event| event.seq).collect::<Vec<_>>(),
            (1..=16).collect::<Vec<_>>()
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
