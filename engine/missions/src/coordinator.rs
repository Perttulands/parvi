//! Owns the mission state directory and its concurrent runs.
use crate::{BindingsSnapshot, Executor, Harnesses, Ledger, RunEvent, Store, run_admission};
use parvi_mission_model::{Finding, RunProjection, RunSessionSummary, RunStart, RunSummary};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug, Default)]
pub struct RunSettings {
    pub run_workspace_root: Option<PathBuf>,
    pub repositories: BTreeMap<String, PathBuf>,
    pub claude_transcripts: Option<PathBuf>,
    pub codex_transcripts: Option<PathBuf>,
    pub pi_transcripts: Option<PathBuf>,
    pub notify_command: Option<PathBuf>,
    /// This node's environment as a step's `device` names it,
    /// `<device>/<environment>`; a step naming another runs there.
    pub environment: String,
    /// Told each time a run blocks, with the run as it then stands and why.
    pub on_block: Option<OnBlock>,
}
/// What a node does when a run blocks: a run waiting on the operator is
/// news on its node.
#[derive(Clone)]
pub struct OnBlock(pub Arc<Tell>);
/// Hears a blocked run and the reason it blocked.
pub type Tell = dyn Fn(&RunProjection, &str) + Send + Sync;
impl std::fmt::Debug for OnBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OnBlock")
    }
}
#[derive(Debug)]
pub enum AdmissionFailure {
    NotFound(String),
    Conflict,
    Invalid(String),
    Findings(Vec<Finding>),
    Io(io::Error),
}
impl From<io::Error> for AdmissionFailure {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl std::fmt::Display for AdmissionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(e) | Self::Invalid(e) => write!(f, "{e}"),
            Self::Conflict => write!(f, "mission changed"),
            Self::Findings(v) => write!(f, "run admission found {} problem(s)", v.len()),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for AdmissionFailure {}
#[derive(Debug, Default)]
pub(crate) struct Control {
    pub canceled: Arc<AtomicBool>,
    pub stopping: Arc<AtomicBool>,
    pub actor: Mutex<String>,
    pub reason: Mutex<String>,
    pub done: Mutex<bool>,
    pub finished: Condvar,
    pub wake: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    pub workspace: Mutex<()>,
    /// Held while a step's limits are read and its start recorded, so two
    /// starts never share a card's last round.
    pub starts: Mutex<()>,
    /// The mission tokens card's pool while the run's worker works.
    pub tokens: Mutex<Option<Arc<crate::attempt_limits::TokenPool>>>,
    /// Where a limit cut off a dispatch reattached before the worker began,
    /// the stop the worker ends the run with.
    pub limit: Mutex<Option<crate::run_limit::LimitStop>>,
}
pub(crate) struct Inner {
    pub root: PathBuf,
    pub settings: RunSettings,
    pub _lock: File,
    pub executor: Arc<dyn Executor>,
    pub admission: Mutex<()>,
    pub shutting_down: AtomicBool,
    pub controls: Mutex<BTreeMap<String, Arc<Control>>>,
    pub recovered: Mutex<BTreeMap<(String, String), crate::FormationResult>>,
    /// Runs whose old ledgers could not be migrated, with why: each refuses
    /// to open, and nothing else does (ledger_migrate.rs).
    pub unmigrated: BTreeMap<String, String>,
    pub subscribers: tokio::sync::broadcast::Sender<RunProjection>,
}
#[derive(Clone)]
pub struct Coordinator {
    pub(crate) inner: Arc<Inner>,
}
impl Coordinator {
    /// The repositories this node's config names, which a step may work in.
    pub fn repositories(&self) -> Vec<String> {
        self.inner.settings.repositories.keys().cloned().collect()
    }
    pub fn seats(&self, id: &str) -> Result<Vec<crate::AgentSeat>, crate::Error> {
        (|| -> io::Result<Vec<crate::AgentSeat>> {
            let events = self.ledger(id)?.read().map_err(crate::error::to_io)?;
            let mut seats = self.inner.executor.seats(id).map_err(crate::error::to_io)?;
            for seat in &mut seats {
                if let Some(waiting) = &mut seat.waiting {
                    if let Some(event) = events.iter().rev().find(|event| {
                        event.kind == "seat_state"
                            && event
                                .data
                                .get("dispatchId")
                                .and_then(|value| value.as_str())
                                == Some(seat.dispatch_id.as_str())
                            && event
                                .data
                                .get("seatEventSeq")
                                .and_then(|value| value.as_u64())
                                == Some(waiting.seq)
                    }) {
                        waiting.seq = event.seq;
                    } else {
                        seat.waiting = None;
                    }
                }
            }
            Ok(seats)
        })()
        .map_err(crate::error::io)
    }
    pub fn open(
        root: impl AsRef<Path>,
        settings: RunSettings,
        executor: Box<dyn Executor>,
    ) -> Result<Self, crate::Error> {
        (|| -> io::Result<Self> {
            let root = root.as_ref();
            if !root.is_absolute()
                || settings
                    .run_workspace_root
                    .as_ref()
                    .is_some_and(|p| !p.is_absolute())
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "state and workspace roots must be absolute",
                ));
            }
            std::fs::create_dir_all(root)?;
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(root.join("coordinator.lock"))?;
            lock.try_lock().map_err(|e| {
                io::Error::other(format!(
                    "another coordinator owns this state directory: {e}"
                ))
            })?;
            std::fs::create_dir_all(root.join("runs"))?;
            let unmigrated = crate::ledger_migrate::migrate(&root.join("runs"))?;
            let (subscribers, _) = tokio::sync::broadcast::channel(128);
            let coordinator = Self {
                inner: Arc::new(Inner {
                    root: root.into(),
                    settings,
                    _lock: lock,
                    executor: Arc::from(executor),
                    admission: Mutex::new(()),
                    shutting_down: AtomicBool::new(false),
                    controls: Mutex::new(BTreeMap::new()),
                    recovered: Mutex::new(BTreeMap::new()),
                    unmigrated,
                    subscribers,
                }),
            };
            coordinator.recover()?;
            Ok(coordinator)
        })()
        .map_err(crate::error::io)
    }
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<RunProjection> {
        self.inner.subscribers.subscribe()
    }
    pub(crate) fn changed(&self, id: &str, seq: u64) {
        if self.inner.subscribers.receiver_count() == 0 {
            return;
        }
        if let Ok(mut events) = self
            .ledger(id)
            .and_then(|ledger| ledger.read().map_err(crate::error::to_io))
        {
            events.truncate(seq as usize);
            if let Ok(projection) = crate::project_run(&events, id) {
                let _ = self.inner.subscribers.send(projection);
            }
        }
    }
    pub(crate) fn ledger(&self, id: &str) -> io::Result<Ledger> {
        if !id.starts_with("run_") || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid run id",
            ));
        }
        if let Some(reason) = self.inner.unmigrated.get(id) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, reason.clone()));
        }
        Ok(Ledger::new(
            self.inner.root.join("runs").join(format!("{id}.ndjson")),
            id,
        ))
    }
    pub fn run(&self, id: &str) -> Result<RunProjection, crate::Error> {
        if self.ledger(id).is_ok_and(|ledger| !ledger.path().exists()) {
            return Err(crate::Error::new(
                "not_found",
                format!("there is no run {id:?} on this node; parvi run list shows its runs"),
            ));
        }
        (|| -> io::Result<RunProjection> {
            crate::project_run(&self.ledger(id)?.read().map_err(crate::error::to_io)?, id)
                .map_err(crate::error::to_io)
        })()
        .map_err(crate::error::io)
    }
    /// The mission as the run froze it when it started: what the run
    /// executes, whatever the editable mission has become since.
    pub fn frozen_mission(&self, id: &str) -> Result<parvi_mission_model::Mission, crate::Error> {
        (|| -> io::Result<parvi_mission_model::Mission> {
            let ledger = self.ledger(id)?;
            if !ledger.path().exists() {
                return Err(io::Error::new(io::ErrorKind::NotFound, "run not found"));
            }
            let path = self
                .inner
                .root
                .join("runs")
                .join(format!("{id}.snapshot.toml"));
            parvi_mission_model::Mission::parse(&std::fs::read_to_string(path)?)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
        })()
        .map_err(crate::error::io)
    }
    /// The run a start with this client key made, if any.
    fn started_by_request(&self, key: &str) -> Result<Option<String>, AdmissionFailure> {
        for entry in std::fs::read_dir(self.inner.root.join("runs"))? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if let Some(id) = name.strip_suffix(".ndjson")
                && let Ok(p) = self.run(id).map_err(crate::error::to_io)
                && p.request_id == key
            {
                return Ok(Some(id.to_string()));
            }
        }
        Ok(None)
    }
    pub fn events(&self, id: &str) -> Result<Vec<parvi_mission_model::PublicEvent>, crate::Error> {
        (|| -> io::Result<Vec<parvi_mission_model::PublicEvent>> {
            Ok(self.run(id).map_err(crate::error::to_io)?.events)
        })()
        .map_err(crate::error::io)
    }
    pub fn escalations(
        &self,
        id: &str,
    ) -> Result<Vec<parvi_mission_model::Escalation>, crate::Error> {
        (|| -> io::Result<Vec<parvi_mission_model::Escalation>> {
            Ok(crate::project_escalations(
                &self.ledger(id)?.read().map_err(crate::error::to_io)?,
            ))
        })()
        .map_err(crate::error::io)
    }
    pub fn list(
        &self,
        mission: Option<&str>,
        needs_you: bool,
    ) -> Result<Vec<RunSummary>, crate::Error> {
        (|| -> io::Result<Vec<RunSummary>> {
            let filter = mission.map(|name| {
                Store::new(&self.inner.root)
                    .resolve(name)
                    .ok()
                    .and_then(|slug| Store::new(&self.inner.root).read_mission(&slug).ok())
                    .map(|m| m.id)
            });
            let mut views = Vec::new();
            for entry in std::fs::read_dir(self.inner.root.join("runs"))? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if let Some(id) = name.strip_suffix(".ndjson")
                    && let Ok(p) = self.run(id).map_err(crate::error::to_io)
                {
                    views.push(p);
                }
            }
            views.sort_by(|a, b| {
                let parse = |value: &str| {
                    time::OffsetDateTime::parse(
                        value,
                        &time::format_description::well_known::Rfc3339,
                    )
                    .ok()
                };
                (parse(&a.started_at), &a.run_id).cmp(&(parse(&b.started_at), &b.run_id))
            });
            let mut counts = BTreeMap::new();
            let mut summaries = Vec::new();
            for p in views {
                let number = counts.entry(p.mission_id.clone()).or_insert(0);
                *number += 1;
                if let Some(expected) = &filter
                    && expected.as_ref() != Some(&p.mission_id)
                {
                    continue;
                }
                if needs_you
                    && (p.r#final || !["blocked", "waiting_human"].contains(&p.status.as_str()))
                {
                    continue;
                }
                let text = p
                    .inputs
                    .iter()
                    .find(|i| i.kind == "text")
                    .or_else(|| p.inputs.first())
                    .map(|i| i.value.split_whitespace().collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                let mut chars = text.chars();
                let mut excerpt: String = chars.by_ref().take(90).collect();
                if chars.next().is_some() {
                    excerpt = excerpt.chars().take(89).collect();
                    excerpt.push('…');
                }
                let mut sessions: Vec<RunSessionSummary> = Vec::new();
                if !p.r#final {
                    for event in &p.events {
                        if event.kind == "seat_created"
                            && !event.session_name.is_empty()
                            && !event.node_id.is_empty()
                        {
                            if !sessions
                                .iter()
                                .any(|s| s.session_name == event.session_name)
                            {
                                sessions.push(RunSessionSummary {
                                    session_name: event.session_name.clone(),
                                    node_id: event.node_id.clone(),
                                    slot_id: event.slot_id.clone(),
                                });
                            }
                        } else if event.kind == "seat_cleanup"
                            && ["ended", "gone"].contains(&event.outcome.as_str())
                        {
                            sessions.retain(|s| s.session_name != event.session_name);
                        }
                    }
                }
                summaries.push(RunSummary {
                    run_id: p.run_id,
                    number: *number,
                    status: p.status,
                    r#final: p.r#final,
                    mission_slug: p.mission_slug,
                    mission_id: p.mission_id,
                    mission_rev: p.mission_rev,
                    input_card_id: p.input_card_id,
                    work: p.work,
                    event_count: p.event_count,
                    resume_allowed: p.resume_allowed,
                    resume_policy: p.resume_policy,
                    block_code: p.block_code,
                    started_by: p.started_by,
                    started_at: p.started_at,
                    updated_at: p.updated_at,
                    input_excerpt: excerpt,
                    outputs: p.outputs,
                    sessions,
                });
            }
            Ok(summaries)
        })()
        .map_err(crate::error::io)
    }
    pub fn start_mission(&self, request: RunStart) -> Result<String, AdmissionFailure> {
        self.start(request, false)
    }
    pub fn start_formation(&self, request: RunStart) -> Result<String, AdmissionFailure> {
        self.start(request, true)
    }
    fn start(&self, mut request: RunStart, single: bool) -> Result<String, AdmissionFailure> {
        let _admission = self.inner.admission.lock().unwrap();
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(AdmissionFailure::Invalid(
                "coordinator is shutting down".into(),
            ));
        }
        // Admission is serialized, so a repeat of a start still being
        // admitted waits here and then finds the run it made.
        if !request.request_id.is_empty()
            && let Some(id) = self.started_by_request(&request.request_id)?
        {
            return Ok(id);
        }
        let store = Store::new(&self.inner.root);
        let slug = store
            .resolve(&request.mission)
            .map_err(|e| AdmissionFailure::NotFound(e.to_string()))?;
        let locks = crate::write::locks(&self.inner.root, &format!("mission:{slug}"));
        let _references = locks.references.read().unwrap();
        let _document = locks.document.lock().unwrap();
        let edit = store
            .mission_edit(&slug)
            .map_err(|e| AdmissionFailure::NotFound(e.to_string()))?;
        let mission = edit.mission;
        if request.expected_rev != mission.rev
            || (!request.expected_etag.is_empty() && request.expected_etag != edit.etag)
        {
            return Err(AdmissionFailure::Conflict);
        }
        let card = mission
            .input_card()
            .ok_or_else(|| AdmissionFailure::NotFound("input card not found".into()))?;
        if single {
            if mission.formation(&request.formation_id).is_none() {
                return Err(AdmissionFailure::NotFound("formation not found".into()));
            }
            request.input_card_id = card.id.clone();
        } else if card.id != request.input_card_id {
            return Err(AdmissionFailure::NotFound("input card not found".into()));
        }
        let root = if single {
            &request.formation_id
        } else {
            &request.input_card_id
        };
        let report = run_admission(
            &mission,
            &store.roles(),
            &Harnesses::from_host(),
            root,
            single,
            &request.inputs,
        );
        if !report.errors.is_empty() {
            return Err(AdmissionFailure::Findings(report.errors));
        }
        crate::run_storage::validate_request(&request)?;
        let mut repositories = std::collections::BTreeSet::new();
        if !request.repository.is_empty() {
            repositories.insert(request.repository.clone());
        }
        for formation in &mission.formations {
            if single && formation.id != request.formation_id {
                continue;
            }
            if let Some(brief) = &formation.brief
                && !brief.repository.is_empty()
            {
                repositories.insert(brief.repository.clone());
            }
        }
        let missing: Vec<_> = repositories
            .iter()
            .filter(|repo| {
                !self
                    .inner
                    .settings
                    .repositories
                    .get(*repo)
                    .is_some_and(|path| path.is_dir())
            })
            .map(|repo| {
                Finding::new(
                    parvi_mission_model::Code::NoCheckout,
                    "",
                    format!(
                        "No repository named \"{repo}\" is set up on this node. \
                         Pick one the node knows, or add it under [missions] repositories \
                         in the node's config."
                    ),
                )
            })
            .collect();
        if !missing.is_empty() {
            return Err(AdmissionFailure::Findings(missing));
        }
        if repositories.len() > 1 {
            return Err(AdmissionFailure::Invalid(
                "a run requires one repository".into(),
            ));
        }
        if request.repository.is_empty() {
            request.repository = repositories.into_iter().next().unwrap_or_default();
        }
        let unknown: Vec<_> = mission
            .formations
            .iter()
            .filter(|f| !single || f.id == request.formation_id)
            .filter(|f| !f.device.is_empty() && f.device != self.inner.settings.environment)
            .filter(|f| {
                let device = f.device.split('/').next().unwrap_or("");
                request.known_devices.as_ref().is_some_and(|known| {
                    !known.iter().any(|k| k == device)
                })
            })
            .map(|f| {
                Finding::new(
                    parvi_mission_model::Code::UnknownDevice,
                    &f.id,
                    format!(
                        "formation {:?} runs on {}, a device this node does not know; name one of the fleet's devices or clear it",
                        f.id, f.device
                    ),
                )
            })
            .collect();
        if !unknown.is_empty() {
            return Err(AdmissionFailure::Findings(unknown));
        }
        // A step placed on another environment reaches it through the run
        // branch, so a run without a repository cannot place one.
        let unplaceable: Vec<_> = mission
            .formations
            .iter()
            .filter(|f| !single || f.id == request.formation_id)
            .filter(|f| !f.device.is_empty() && f.device != self.inner.settings.environment)
            .filter(|_| request.repository.is_empty())
            .map(|f| {
                Finding::new(
                    parvi_mission_model::Code::PlacementWithoutRepository,
                    &f.id,
                    format!(
                        "formation {:?} runs on {}, which reaches the run's work only through a repository; give the run a repository or clear the device",
                        f.id, f.device
                    ),
                )
            })
            .collect();
        if !unplaceable.is_empty() {
            return Err(AdmissionFailure::Findings(unplaceable));
        }
        if !request.repository.is_empty() && !request.cwd.is_empty() {
            return Err(AdmissionFailure::Invalid(
                "repository and cwd cannot be combined".into(),
            ));
        }
        if request.repository.is_empty() && !request.base.is_empty() {
            return Err(AdmissionFailure::Invalid(
                "base requires a repository".into(),
            ));
        }
        let id = crate::run_storage::run_id()?;
        let bindings =
            BindingsSnapshot::capture(&id, &mission, &request.input_card_id, &store.roles())
                .map_err(|error| AdmissionFailure::Invalid(error.message))?;
        let workspace =
            crate::run_storage::create(self, &id, &slug, &mission, &bindings, &request)?;
        let control = Arc::new(Control::default());
        self.inner
            .controls
            .lock()
            .unwrap()
            .insert(id.clone(), control.clone());
        self.changed(&id, 1);
        let owner = self.clone();
        let worker_id = id.clone();
        std::thread::spawn(move || {
            crate::run_worker::execute(
                owner, worker_id, mission, bindings, request, workspace, control,
            );
        });
        Ok(id)
    }
    /// Removes a final run's worktree, branch and shared refs; returns a
    /// note for each shared ref it left because it moved.
    pub fn land(&self, id: &str, force: bool) -> Result<Vec<String>, crate::Error> {
        (|| -> io::Result<Vec<String>> {
            let _admission = self.inner.admission.lock().unwrap();
            let projection = self.run(id).map_err(crate::error::to_io)?;
            if !projection.r#final {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "run is not final",
                ));
            }
            let events = self.ledger(id)?.read().map_err(crate::error::to_io)?;
            let first = &events[0];
            let get = |key: &str| {
                first
                    .data
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned()
            };
            let checkout = get("checkout");
            if checkout.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "run has no worktree",
                ));
            }
            let tree = crate::Worktree {
                path: PathBuf::from(get("cwd")),
                checkout: PathBuf::from(checkout),
                branch: format!("parvi/run-{id}"),
                base: get("base"),
            };
            // What each placed turn's node pushed, so only that is deleted.
            let published = events
                .iter()
                .filter(|e| e.kind == "workspace_turn")
                .filter_map(|e| {
                    let field = |name| e.data.get(name)?.as_str().map(str::to_owned);
                    Some((field("reference")?, field("commit")?))
                })
                .collect();
            crate::Workspaces::new(
                self.inner.root.join("worktrees"),
                self.inner.settings.repositories.clone(),
            )
            .remove(&tree, force, &published)
            .map_err(crate::error::to_io)
        })()
        .map_err(crate::error::io)
    }
    /// Waits until execution has stopped and records cancellation only after cleanup.
    pub fn abort(
        &self,
        id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<RunProjection, crate::Error> {
        (|| -> io::Result<RunProjection> {
            let control = self.inner.controls.lock().unwrap().get(id).cloned();
            if let Some(control) = control {
                let mut done = control.done.lock().unwrap();
                if *done || control.canceled.load(Ordering::Acquire) {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "run is final or aborting",
                    ));
                }
                *control.actor.lock().unwrap() = if actor.is_empty() {
                    crate::DEFAULT_ACTOR.into()
                } else {
                    actor.into()
                };
                *control.reason.lock().unwrap() = reason.to_owned();
                control.canceled.store(true, Ordering::Release);
                if let Some(wake) = control.wake.lock().unwrap().as_ref() {
                    let _ = wake.send(());
                }
                while !*done {
                    done = control.finished.wait(done).unwrap();
                }
            } else {
                let projection = self.run(id).map_err(crate::error::to_io)?;
                if projection.r#final {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, "run is final"));
                } else {
                    crate::run_left_seats::end(self, id, None, "run_final")?;
                    self.append(
                        id,
                        RunEvent {
                            kind: "run_canceled".into(),
                            actor: if actor.is_empty() {
                                crate::DEFAULT_ACTOR.into()
                            } else {
                                actor.into()
                            },
                            timestamp: crate::run_storage::now(),
                            data: serde_json::json!({"reason":reason,"final":true})
                                .as_object()
                                .unwrap()
                                .clone(),
                            ..Default::default()
                        },
                    )?;
                }
            }
            self.inner
                .recovered
                .lock()
                .unwrap()
                .retain(|(run, _), _| run != id);
            self.run(id).map_err(crate::error::to_io)
        })()
        .map_err(crate::error::io)
    }
    pub(crate) fn append(&self, id: &str, event: RunEvent) -> io::Result<()> {
        let event = self
            .ledger(id)?
            .append(event)
            .map_err(crate::error::to_io)?;
        self.changed(id, event.seq);
        if event.kind == "run_blocked"
            && let Some(OnBlock(tell)) = &self.inner.settings.on_block
            && let Ok(run) = self.run(id).map_err(crate::error::to_io)
        {
            let reason = event.data.get("reason").and_then(|v| v.as_str());
            tell(&run, reason.unwrap_or(&run.block_code));
        }
        Ok(())
    }
}
