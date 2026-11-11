//! Scheduled prompts (#189): tasks that paste a prompt into named sessions
//! on a schedule, CHROTE's scheduler ported (`src/internal/scheduled/`,
//! `docs/scheduled-tasks.md`). The API is CHROTE's, envelope, headers and
//! all, so the `scheduled-tasks` skill works against a node by changing only
//! its base URL.
//!
//! What differs: CHROTE re-reads every task every 30 seconds. Here the node
//! keeps when each task is next due, and its one timer sleeps until the
//! earliest of them; with nothing scheduled there is no timer at all. Task
//! files are read when the API is asked and when a task falls due. A task's
//! targets are this node's sessions.
//!
//! A run is never repeated: it is written ahead into the task's file before
//! anything is sent, and nothing is sent if that write fails. A task whose
//! file will not read, whose run could not be written ahead or recorded, or
//! which the node finds still mid-run as it starts, is held back (`held` in
//! the API, an event on the Fleet floor) until it is edited or resumed. A due
//! task another call holds waits for that claim to be given back, which
//! wakes the timer.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use jiff::Timestamp;
use parvi_node_kit::BoxFuture;
use parvi_node_kit::events::Log;
use parvi_scheduled_contract::{
    Attempt, AuditEntry, BAD_REQUEST, CONFLICT, ERROR, INTERNAL, NOT_FOUND, PARTIAL, RunEntry,
    SUBMIT_KEY_DISPATCHED, SUCCESS, Schedule, TARGET_NOT_FOUND, Target, TargetRun, Task,
    normalize_targets,
};
use parvi_term::Sessions;
use tokio::sync::{Notify, Semaphore, watch};

use crate::schedule;
use crate::store::{self, Store};

/// The kind of event the area records when it holds a task back.
const EVENT: &str = "scheduled";

const MAX_RECENT_RUNS: usize = 20;
const MAX_AUDIT: usize = 50;
const RUN_TIMEOUT: Duration = Duration::from_secs(5);
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TARGETS: usize = 32;
const MAX_PROMPT: usize = 64 * 1024;
/// Deliveries of one run at once, as CHROTE's production service allows.
const CONCURRENT_DELIVERIES: usize = 8;
/// The longest the timer sleeps while a task waits: a machine that slept
/// through a due time fires it within this of waking, since the timer's
/// clock does not count the sleep.
const LONGEST_SLEEP: Duration = Duration::from_secs(300);

/// A schedule or task that cannot be kept, in CHROTE's words.
#[derive(Debug, Clone, PartialEq)]
pub struct Invalid(String);

impl Invalid {
    pub fn new(m: impl Into<String>) -> Invalid {
        Invalid(m.into())
    }
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid scheduled task: {}", self.0)
    }
}

#[derive(Debug)]
pub enum Error {
    NotFound,
    Invalid(Invalid),
    TargetNotFound(String),
    Conflict(String),
    Internal(anyhow::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound => f.write_str("scheduled task not found"),
            Error::Invalid(i) => i.fmt(f),
            Error::TargetNotFound(m) => write!(f, "scheduled task target not found: {m}"),
            Error::Conflict(id) => write!(f, "scheduled task is already being modified: {id}"),
            Error::Internal(e) => write!(f, "{e:#}"),
        }
    }
}

impl From<Invalid> for Error {
    fn from(i: Invalid) -> Error {
        Error::Invalid(i)
    }
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Error {
        Error::Internal(e)
    }
}

impl From<Error> for parvi_core::Error {
    fn from(e: Error) -> parvi_core::Error {
        let code = match &e {
            Error::NotFound => NOT_FOUND,
            Error::Invalid(_) => BAD_REQUEST,
            Error::TargetNotFound(_) => TARGET_NOT_FOUND,
            Error::Conflict(_) => CONFLICT,
            Error::Internal(_) => INTERNAL,
        };
        parvi_core::Error::new(code, e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Checks targets and pastes prompts into them.
pub trait Runner: Send + Sync {
    fn validate<'a>(&'a self, target: &'a Target) -> BoxFuture<'a, Result<(), String>>;
    /// Pastes the prompt and presses Enter; the pane it went to. A send
    /// whose turn does not come within `deadline` is withdrawn unsent; one
    /// under way is not given up as failed while it may still paste.
    fn send<'a>(
        &'a self,
        target: &'a Target,
        prompt: &'a str,
        deadline: Duration,
    ) -> BoxFuture<'a, Result<String, String>>;
}

#[derive(Default)]
pub struct Create {
    pub name: String,
    pub prompt: String,
    pub targets: Vec<Target>,
    pub schedule: Schedule,
    pub enabled: Option<bool>,
    pub paused: bool,
    pub created_by: String,
    pub updated_by: String,
}

#[derive(Default)]
pub struct Patch {
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub targets: Option<Vec<Target>>,
    pub schedule: Option<Schedule>,
    pub enabled: Option<bool>,
    pub paused: Option<bool>,
    pub updated_by: String,
}

pub struct Scheduled {
    events: Arc<Log>,
    store: Store,
    runner: Arc<dyn Runner>,
    now: Box<dyn Fn() -> Timestamp + Send + Sync>,
    validate_targets: bool,
    run_timeout: Duration,
    /// Tasks whose mutation or run is under way: one at a time each.
    claims: Mutex<HashSet<String>>,
    /// When each waiting task is next due; the timer sleeps to the earliest.
    due: Mutex<HashMap<String, Timestamp>>,
    /// Tasks held back, and why: out of `due` until saved again.
    held: Mutex<HashMap<String, String>>,
    changed: Notify,
    updates: watch::Sender<u64>,
}

/// A task's claim, given back when dropped.
struct Claim<'a> {
    owner: &'a Scheduled,
    id: String,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.owner.claims.lock().unwrap().remove(&self.id);
        // A due task the timer passed over while this was held is due now.
        self.owner.changed.notify_one();
    }
}

impl Scheduled {
    pub fn with_events(mut self, events: Arc<Log>) -> Self {
        self.events = events;
        self
    }

    pub fn new(dir: PathBuf, runner: Arc<dyn Runner>) -> Scheduled {
        Scheduled {
            events: Default::default(),
            store: Store::new(dir),
            runner,
            now: Box::new(Timestamp::now),
            validate_targets: true,
            run_timeout: RUN_TIMEOUT,
            claims: Mutex::default(),
            due: Mutex::default(),
            held: Mutex::default(),
            changed: Notify::new(),
            updates: watch::Sender::new(0),
        }
    }

    fn now(&self) -> Timestamp {
        (self.now)()
    }

    fn claim(&self, id: &str) -> Result<Claim<'_>> {
        if !self.claims.lock().unwrap().insert(id.to_string()) {
            return Err(Error::Conflict(id.to_string()));
        }
        Ok(Claim {
            owner: self,
            id: id.to_string(),
        })
    }

    fn get_task(&self, id: &str) -> Result<Task> {
        if !store::safe_id(id) {
            return Err(Invalid::new("invalid task id").into());
        }
        self.store.get(id)?.ok_or(Error::NotFound)
    }

    /// Writes the task and tells the timer when it is next due.
    fn save(&self, task: &Task) -> Result<()> {
        self.store.save(task)?;
        self.held.lock().unwrap().remove(&task.id);
        self.track(task);
        self.announce();
        Ok(())
    }

    fn announce(&self) {
        self.updates.send_modify(|n| *n = n.wrapping_add(1));
    }

    pub(crate) fn updates(&self) -> watch::Receiver<u64> {
        self.updates.subscribe()
    }

    /// Holds a task back: it will not fire until saved again, and the
    /// operator is told.
    fn hold(&self, id: &str, why: String) {
        self.due.lock().unwrap().remove(id);
        self.events
            .record(EVENT, true, format!("Scheduled task {id} held back: {why}"));
        eprintln!("parvi: scheduled task {id} held back: {why}");
        self.held.lock().unwrap().insert(id.to_string(), why);
        self.announce();
        self.changed.notify_one();
    }

    /// The task as the API says it: with why it is held, if it is.
    fn said(&self, mut task: Task) -> Task {
        if let Some(why) = self.held.lock().unwrap().get(&task.id) {
            task.held = why.clone();
        }
        task
    }

    fn track(&self, task: &Task) {
        let mut due = self.due.lock().unwrap();
        match task.next_run.filter(|_| task.enabled && !task.paused) {
            Some(at) => due.insert(task.id.clone(), at),
            None => due.remove(&task.id),
        };
        drop(due);
        self.changed.notify_one();
    }

    /// Every task, and after them each file that does not read, said as
    /// a held task of its own so one bad file hides nothing else.
    pub fn list(&self) -> Result<Vec<Task>> {
        let (tasks, broken) = self.store.list_all()?;
        Ok(tasks
            .into_iter()
            .map(|t| self.said(t))
            .chain(broken.into_iter().map(|(id, e)| unreadable(id, &e)))
            .collect())
    }

    pub fn get(&self, id: &str) -> Result<Task> {
        self.get_task(id).map(|t| self.said(t))
    }

    /// Points every target that named `from` on `user`'s server ("" for the
    /// node's own) at `to`, so a renamed session keeps its tasks (#237).
    /// A task mid-run is rewritten once its run gives it back. Answers the
    /// tasks it rewrote.
    pub async fn follow_rename(&self, user: &str, from: &str, to: &str) -> Result<Vec<String>> {
        let own = crate::runner::own_user();
        let names = |t: &Target| {
            t.session_name == from
                && (t.unix_user == user || (user.is_empty() && t.unix_user == own))
        };
        let mut rewrote = vec![];
        for listed in self.store.list_all()?.0 {
            if !listed.targets.iter().any(names) {
                continue;
            }
            let claim = loop {
                match self.claim(&listed.id) {
                    Ok(claim) => break claim,
                    Err(Error::Conflict(_)) => tokio::time::sleep(Duration::from_millis(500)).await,
                    Err(e) => return Err(e),
                }
            };
            let mut task = self.get_task(&listed.id)?;
            for target in task.targets.iter_mut().filter(|t| names(t)) {
                target.session_name = to.to_string();
            }
            let now = self.now();
            task.updated_at = now;
            audit(
                &mut task,
                now,
                "parvi",
                "updated",
                &format!("session {from} renamed to {to}"),
            );
            self.save(&task)?;
            drop(claim);
            rewrote.push(task.id);
        }
        Ok(rewrote)
    }

    /// Follows every session rename on the sessions' servers while the node
    /// runs: an event, so nothing runs while no one renames.
    pub fn follow_renames(
        self: &Arc<Self>,
        sessions: &dyn Sessions,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        (sessions.renames().into_iter())
            .map(|(user, mut renames)| {
                let scheduled = self.clone();
                let user = user.unwrap_or_default();
                tokio::spawn(async move {
                    loop {
                        let (from, to) = match renames.recv().await {
                            Ok(renamed) => renamed,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                scheduled.events.record(
                                    EVENT,
                                    true,
                                    format!("{n} session renames came too fast to follow; check Scheduled's targets"),
                                );
                                continue;
                            }
                            Err(_) => return,
                        };
                        if let Err(e) = scheduled.follow_rename(&user, &from, &to).await {
                            scheduled.events.record(
                                EVENT,
                                true,
                                format!("Scheduled tasks did not follow {from} renamed to {to}: {e}"),
                            );
                        }
                    }
                })
            })
            .collect()
    }

    pub async fn create(&self, r: Create) -> Result<Task> {
        let now = self.now();
        let created_by = r.created_by.trim().to_string();
        let updated_by = match r.updated_by.trim() {
            "" => created_by.clone(),
            u => u.to_string(),
        };
        let mut task = Task {
            id: new_id("tsk", now),
            name: r.name.trim().to_string(),
            prompt: r.prompt,
            targets: normalize_targets(r.targets),
            schedule: r.schedule,
            enabled: r.enabled.unwrap_or(true),
            paused: r.paused,
            next_run: None,
            last_run: None,
            last_status: String::new(),
            created_by,
            updated_by: updated_by.clone(),
            created_at: now,
            updated_at: now,
            recent_runs: vec![],
            audit: vec![],
            held: String::new(),
            attempt: None,
        };
        self.prepare(&mut task, true, now).await?;
        audit(&mut task, now, &updated_by, "created", "");
        self.save(&task)?;
        Ok(task)
    }

    pub async fn patch(&self, id: &str, r: Patch) -> Result<Task> {
        let _claim = self.claim(id)?;
        let mut task = self.get_task(id)?;
        let now = self.now();
        let targets_changed = r.targets.is_some();
        if let Some(name) = r.name {
            task.name = name.trim().to_string();
        }
        if let Some(prompt) = r.prompt {
            task.prompt = prompt;
        }
        if let Some(targets) = r.targets {
            task.targets = normalize_targets(targets);
        }
        if let Some(schedule) = r.schedule {
            task.schedule = schedule;
        }
        if let Some(enabled) = r.enabled {
            task.enabled = enabled;
        }
        if let Some(paused) = r.paused {
            task.paused = paused;
        }
        if !r.updated_by.is_empty() {
            task.updated_by = r.updated_by.trim().to_string();
        }
        task.updated_at = now;
        task.attempt = None;
        self.prepare(&mut task, targets_changed, now).await?;
        let actor = task.updated_by.clone();
        audit(&mut task, now, &actor, "updated", "");
        self.save(&task)?;
        Ok(task)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let _claim = self.claim(id)?;
        if !store::safe_id(id) {
            return Err(Invalid::new("invalid task id").into());
        }
        if !self.store.delete(id)? {
            return Err(Error::NotFound);
        }
        self.due.lock().unwrap().remove(id);
        self.changed.notify_one();
        self.announce();
        Ok(())
    }

    /// Paused keeps the task enabled and clears its next run.
    pub fn pause(&self, id: &str, actor: &str) -> Result<Task> {
        let _claim = self.claim(id)?;
        let mut task = self.get_task(id)?;
        let now = self.now();
        task.paused = true;
        task.next_run = None;
        task.updated_at = now;
        task.attempt = None;
        if !actor.trim().is_empty() {
            task.updated_by = actor.trim().to_string();
        }
        let actor = task.updated_by.clone();
        audit(&mut task, now, &actor, "paused", "");
        self.save(&task)?;
        Ok(task)
    }

    pub async fn resume(&self, id: &str, actor: &str) -> Result<Task> {
        let _claim = self.claim(id)?;
        let mut task = self.get_task(id)?;
        let now = self.now();
        task.paused = false;
        task.updated_at = now;
        task.attempt = None;
        if !actor.trim().is_empty() {
            task.updated_by = actor.trim().to_string();
        }
        self.prepare(&mut task, false, now).await?;
        let actor = task.updated_by.clone();
        audit(&mut task, now, &actor, "resumed", "");
        self.save(&task)?;
        Ok(task)
    }

    /// Sends the prompt now and records the run; the schedule stays.
    pub async fn run_now(&self, id: &str, actor: &str) -> Result<(Task, RunEntry)> {
        let _claim = self.claim(id)?;
        let mut task = self.get_task(id)?;
        let run = match self.fire(&mut task, "manual").await {
            Ok(run) => run,
            Err(e) => {
                self.hold(id, format!("{NOT_SENT}: {e}"));
                return Err(e);
            }
        };
        let now = self.now();
        if !actor.trim().is_empty() {
            task.updated_by = actor.trim().to_string();
        }
        task.updated_at = now;
        let actor = task.updated_by.clone();
        audit(&mut task, now, &actor, "run-now", &run.status.clone());
        if let Err(e) = self.save(&task) {
            self.hold(
                id,
                format!(
                    "a manual run ran ({}) but its record could not be saved: {e}",
                    run.status
                ),
            );
            return Err(e);
        }
        Ok((task, run))
    }

    /// Reads every task once, as the node starts: gives an enabled task
    /// without a next run its next run, and learns when each is due. A task
    /// that does not read, cannot be saved, or was mid-run when the node
    /// stopped is held, and the rest start; only a folder that cannot be
    /// read stops the node.
    pub fn prime(&self) -> anyhow::Result<()> {
        let now = self.now();
        let (tasks, broken) = self
            .store
            .list_all()
            .context("reading the scheduled tasks")?;
        for (id, e) in broken {
            self.hold(&id, unreadable_reason(&e));
        }
        for mut task in tasks {
            if let Some(attempt) = &task.attempt {
                self.hold(&task.id, unconfirmed(attempt));
                continue;
            }
            if task.enabled
                && !task.paused
                && task.next_run.is_none()
                && let Ok(next) = schedule::next_run(&task.schedule, now)
            {
                task.next_run = Some(next);
                if let Err(e) = self.save(&task) {
                    self.hold(&task.id, format!("its next run could not be saved: {e}"));
                }
                continue;
            }
            self.track(&task);
        }
        Ok(())
    }

    /// Fires every task due by now, each under its claim; a task claimed by
    /// a mutation or a manual run is left for that to finish.
    pub async fn run_due(&self) -> Result<Vec<RunEntry>> {
        let now = self.now();
        let due: Vec<String> = {
            let due = self.due.lock().unwrap();
            let mut ids: Vec<(&String, &Timestamp)> =
                due.iter().filter(|(_, at)| **at <= now).collect();
            ids.sort_by_key(|(id, at)| (**at, (*id).clone()));
            ids.into_iter().map(|(id, _)| id.clone()).collect()
        };
        let mut runs = vec![];
        let mut failed = None;
        for id in due {
            // A claimed task is left to its claim, whose release wakes the
            // timer again.
            let Ok(_claim) = self.claim(&id) else {
                continue;
            };
            // One task that fails never holds up the rest.
            match self.run_due_task(&id, now).await {
                Ok(run) => runs.extend(run),
                Err(e) => failed = Some(e),
            }
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(runs),
        }
    }

    async fn run_due_task(&self, id: &str, now: Timestamp) -> Result<Option<RunEntry>> {
        let mut task = match self.store.get(id) {
            Ok(Some(task)) => task,
            Ok(None) => {
                self.due.lock().unwrap().remove(id);
                return Ok(None);
            }
            Err(e) => {
                // Read again on every wake it would only fail again.
                self.hold(id, unreadable_reason(&e));
                return Err(e.into());
            }
        };
        if let Some(attempt) = &task.attempt {
            self.hold(id, unconfirmed(attempt));
            return Ok(None);
        }
        let was = task.updated_at;
        let due = task.enabled && !task.paused && task.next_run.is_some_and(|at| at <= now);
        if !due {
            self.track(&task);
            return Ok(None);
        }
        let mut run = None;
        match self.prepare(&mut task, false, now).await {
            Err(e) => {
                task.last_status = ERROR.into();
                task.updated_at = now;
                // Not CHROTE: its 30 s tick tries again; this timer would
                // spin on a time in the past, so the task waits for its
                // next time, or for an edit when its schedule does not read.
                task.next_run = schedule::next_run(&task.schedule, now).ok();
                audit(&mut task, now, "", "scheduler-error", &e.to_string());
            }
            Ok(()) => {
                let fired = match self.fire(&mut task, "scheduled").await {
                    Ok(fired) => fired,
                    Err(e) => {
                        self.hold(id, format!("{NOT_SENT}: {e}"));
                        return Err(e);
                    }
                };
                task.updated_at = now;
                audit(&mut task, now, "", "scheduled-run", &fired.status.clone());
                run = Some(fired);
            }
        }
        // What was sent is never sent again for the same due time: if the
        // run cannot be recorded, the task is held back instead.
        let sent = |run: &Option<RunEntry>| {
            run.as_ref().map_or("it did not run".to_string(), |r| {
                format!("it ran ({})", r.status)
            })
        };
        // A task deleted while it fired stays deleted; one changed outside
        // its claim (by hand, in its file) is not overwritten.
        match self.store.get(id) {
            Ok(None) => {
                self.due.lock().unwrap().remove(id);
                return Ok(run);
            }
            Ok(Some(latest)) if latest.updated_at != was => {
                if latest.next_run.is_some_and(|at| at <= now) {
                    self.hold(
                        id,
                        format!("{} while its file was changed by hand", sent(&run)),
                    );
                } else {
                    self.track(&latest);
                }
                return Err(Error::Conflict(format!(
                    "{id} changed outside its task claim"
                )));
            }
            Ok(Some(_)) => {}
            Err(e) => {
                self.hold(
                    id,
                    format!("{} but its file then did not read: {e:#}", sent(&run)),
                );
                return Err(e.into());
            }
        }
        if let Err(e) = self.save(&task) {
            self.hold(
                id,
                format!("{} but its record could not be saved: {e}", sent(&run)),
            );
            return Err(e);
        }
        Ok(run)
    }

    async fn prepare(&self, task: &mut Task, check_targets: bool, now: Timestamp) -> Result<()> {
        let invalid = |m: &str| Err(Error::Invalid(Invalid::new(m)));
        if task.name.is_empty() {
            return invalid("name is required");
        }
        if task.prompt.trim().is_empty() {
            return invalid("prompt is required");
        }
        if task.prompt.contains('\0') {
            return invalid("prompt must not contain NUL bytes");
        }
        if task.prompt.len() > MAX_PROMPT {
            return invalid("prompt is too large");
        }
        task.targets = normalize_targets(std::mem::take(&mut task.targets));
        if task.targets.is_empty() {
            return invalid("at least one target session is required");
        }
        if task.targets.len() > MAX_TARGETS {
            return Err(
                Invalid::new(format!("a task may target at most {MAX_TARGETS} sessions")).into(),
            );
        }
        for t in &task.targets {
            if !safe_session(&t.session_name) {
                return invalid(
                    "target sessionName is required and must contain only letters, numbers, dashes, and underscores",
                );
            }
            if !t.unix_user.is_empty() && !safe_user(&t.unix_user) {
                return invalid("target unixUser contains invalid characters");
            }
        }
        task.schedule = schedule::normalize(task.schedule.clone())?;
        recompute(task, now)?;
        if check_targets && self.validate_targets {
            let gate = Semaphore::new(CONCURRENT_DELIVERIES);
            let checks = task.targets.iter().map(|t| async {
                let _turn = gate.acquire().await;
                self.validate(t).await
            });
            for result in futures_util::future::join_all(checks).await {
                result?;
            }
        }
        Ok(())
    }

    async fn validate(&self, target: &Target) -> Result<()> {
        match tokio::time::timeout(VALIDATE_TIMEOUT, self.runner.validate(target)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(Error::TargetNotFound(e)),
            Err(_) => Err(Error::TargetNotFound("validation took too long".into())),
        }
    }

    /// Writes the attempt ahead, then delivers to every target; one gone
    /// session is recorded against that target alone and never holds up
    /// the rest. If the attempt cannot be written nothing is sent: a send
    /// the node could not remember could be sent again after a restart.
    async fn fire(&self, task: &mut Task, trigger: &str) -> Result<RunEntry> {
        let started = self.now();
        let id = new_id("run", started);
        task.attempt = Some(Attempt {
            run_id: id.clone(),
            trigger: trigger.into(),
            started_at: started,
        });
        if let Err(e) = self.store.save(task) {
            task.attempt = None;
            return Err(e.into());
        }
        let mut run = RunEntry {
            id,
            trigger: trigger.into(),
            started_at: started,
            finished_at: started,
            status: String::new(),
            message: String::new(),
            targets: vec![],
        };
        let targets = normalize_targets(task.targets.clone());
        if targets.is_empty() {
            run.status = ERROR.into();
            run.message = "task has no targets".into();
        } else {
            let gate = Semaphore::new(CONCURRENT_DELIVERIES);
            let deliveries = targets.iter().map(|t| async {
                let _turn = gate.acquire().await;
                self.deliver(t, &task.prompt).await
            });
            run.targets = futures_util::future::join_all(deliveries).await;
            let failures: Vec<String> = run
                .targets
                .iter()
                .zip(&targets)
                .filter(|(r, _)| r.status != SUCCESS)
                .map(|(r, t)| format!("{}: {}", t.label(), r.message))
                .collect();
            run.status = match failures.len() {
                0 => SUCCESS,
                n if n == targets.len() => ERROR,
                _ => PARTIAL,
            }
            .into();
            run.message = failures.join("; ");
        }
        let finished = self.now();
        run.finished_at = finished;
        task.last_run = Some(finished);
        task.last_status = run.status.clone();
        task.recent_runs.insert(0, run.clone());
        task.recent_runs.truncate(MAX_RECENT_RUNS);
        task.attempt = None;
        let _ = recompute(task, finished);
        Ok(run)
    }

    /// One target, validation and paste within one deadline. The runner
    /// owns the deadline once the paste is queued, so a send given up on is
    /// withdrawn, never left to paste after a failure was recorded.
    async fn deliver(&self, target: &Target, prompt: &str) -> TargetRun {
        let mut result = TargetRun {
            session_name: target.session_name.clone(),
            unix_user: target.unix_user.clone(),
            status: SUCCESS.into(),
            ..TargetRun::default()
        };
        let start = tokio::time::Instant::now();
        let exceeded = || {
            format!(
                "delivery deadline exceeded after {} s",
                self.run_timeout.as_secs_f32()
            )
        };
        let attempt = async {
            if self.validate_targets {
                match tokio::time::timeout(self.run_timeout, self.validate(target)).await {
                    Err(_) => return Err(exceeded()),
                    Ok(Err(e)) => return Err(e.to_string()),
                    Ok(Ok(())) => {}
                }
            }
            let left = self.run_timeout.saturating_sub(start.elapsed());
            if left.is_zero() {
                return Err(exceeded());
            }
            self.runner.send(target, prompt, left).await
        };
        match attempt.await {
            Ok(pane) => {
                result.pane = pane;
                result.submit_key_dispatched = true;
                result.message = SUBMIT_KEY_DISPATCHED.into();
            }
            Err(e) => {
                result.status = ERROR.into();
                result.message = e;
            }
        }
        result
    }

    /// The node's one scheduling timer: sleeps to the earliest due task,
    /// fires what is due, and sleeps again; wakes early when a task changes.
    /// With nothing waiting it only waits for a change.
    pub async fn run(self: Arc<Self>) {
        loop {
            let changed = self.changed.notified();
            // A claimed task waits for its claim, whose release notifies.
            let next = {
                let claims = self.claims.lock().unwrap();
                let due = self.due.lock().unwrap();
                due.iter()
                    .filter(|(id, _)| !claims.contains(*id))
                    .map(|(_, at)| *at)
                    .min()
            };
            match next {
                None => changed.await,
                Some(at) => {
                    let wait = at
                        .duration_since(self.now())
                        .try_into()
                        .unwrap_or(Duration::ZERO)
                        .min(LONGEST_SLEEP);
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {
                            if let Err(e) = self.run_due().await {
                                eprintln!("parvi: scheduled tasks: {e}");
                            }
                        }
                        _ = changed => {}
                    }
                }
            }
        }
    }
}

/// Why a task whose run was not written ahead is held.
const NOT_SENT: &str = "its run could not be written ahead, so nothing was sent";

fn unreadable_reason(e: &anyhow::Error) -> String {
    format!("its file does not read: {e:#}")
}

/// Why a task found mid-run as the node starts is held, not sent again.
fn unconfirmed(a: &Attempt) -> String {
    format!(
        "unconfirmed: a {} run began at {} and was never recorded, so it may or may not have reached its targets; check the agent before editing or resuming the task",
        a.trigger, a.started_at
    )
}

/// A file that does not read, listed as a held task of its own.
fn unreadable(id: String, e: &anyhow::Error) -> Task {
    Task {
        name: format!("Unreadable task {id}"),
        id,
        prompt: String::new(),
        targets: vec![],
        schedule: Schedule::default(),
        enabled: false,
        paused: true,
        next_run: None,
        last_run: None,
        last_status: ERROR.into(),
        created_by: String::new(),
        updated_by: String::new(),
        created_at: Timestamp::UNIX_EPOCH,
        updated_at: Timestamp::UNIX_EPOCH,
        recent_runs: vec![],
        audit: vec![],
        held: unreadable_reason(e),
        attempt: None,
    }
}

fn recompute(task: &mut Task, now: Timestamp) -> Result<(), Invalid> {
    task.next_run = if task.enabled && !task.paused {
        Some(schedule::next_run(&task.schedule, now)?)
    } else {
        None
    };
    Ok(())
}

fn audit(task: &mut Task, at: Timestamp, actor: &str, action: &str, message: &str) {
    task.audit.push(AuditEntry {
        at,
        actor: actor.into(),
        action: action.into(),
        message: message.into(),
    });
    let over = task.audit.len().saturating_sub(MAX_AUDIT);
    task.audit.drain(..over);
}

fn safe_session(name: &str) -> bool {
    (1..=50).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn safe_user(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// `tsk_<ms in base 36>_<8 hex>`, CHROTE's shape.
fn new_id(prefix: &str, now: Timestamp) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_i128(now.as_nanosecond());
    let random = h.finish() as u32;
    let mut ms = now.as_millisecond().max(0) as u64;
    let mut base36 = vec![];
    loop {
        base36.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(ms % 36) as usize]);
        ms /= 36;
        if ms == 0 {
            break;
        }
    }
    base36.reverse();
    format!(
        "{prefix}_{}_{random:08x}",
        String::from_utf8(base36).expect("ascii")
    )
}

#[cfg(test)]
mod tests;
