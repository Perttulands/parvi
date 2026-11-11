//! Follows every tmux session through control mode and keeps every pane's
//! screen in memory.
//!
//! One control-mode client per session. Each client attaches with
//! `-f ignore-size` and sends no `refresh-client -C`, so it is never a sizing
//! candidate (issue #4 proved it) until a browser claims the sizing seat. Each pane is
//! owned by the first session that lists it, so a window linked into two
//! sessions is fed once.
//!
//! A pane is seeded from `capture-pane` and then fed every `%output` byte.
//! Output before a capture reply is already in that capture. If output arrives
//! between the capture and cursor replies, the capture is stale: seed again
//! until a capture and its cursor have no intervening output.
//!
//! Clients also attach with `pause-after`, so falling behind a flood costs
//! tmux nothing: it pauses the pane for us instead of buffering its output or
//! dropping the client as too far behind. A paused pane is continued and
//! seeded again. Viewers get the new screen like any reseed.
//!
//! Claiming the sizing seat (ADR-0017 decisions 1 and 2) turns the session's
//! own client into its sizing client: the clients that were sizing the window
//! are set to `ignore-size`, ours drops it and tells tmux the browser's grid.
//! Nobody is detached. Releasing undoes exactly that, so the window goes back to
//! whoever sized it before, or keeps its size if nobody did.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::backend::Backend;
use crate::control::{self, Event, Notification, PaneId};
use crate::utf8::Utf8Gate;

mod delivery;
use delivery::{Deliveries, Delivery, Readiness};
pub use delivery::{Delivering, NotReady, NotSubmitted};

/// How long a delivery into a just-launched pane waits for the pane to get
/// ready for a paste (#242): an agent harness can take seconds to start.
pub const READY_WAIT: Duration = Duration::from_secs(30);

/// How often to look for a tmux server while there isn't one. This is the
/// spike's one timer; watching the socket directory with inotify replaces it.
const NO_SERVER_RETRY: Duration = Duration::from_secs(1);

/// Scrollback lines kept per pane in memory. Zero for now: at vt100's cell
/// size, 50 panes of 200 columns with 1000 lines each would be hundreds of
/// megabytes. Scrollback search is a later design question (issue #3).
const SCROLLBACK_LINES: usize = 0;

/// The largest grid a claim may ask for. tmux refuses more than 10000.
const MAX_GRID: u16 = 10000;

/// A session counts as active for this long after its last output.
const ACTIVE_FOR: Duration = Duration::from_secs(5);

/// After output, how long until the screens are read for a waiting prompt.
/// At most one look per this long while output flows, and always one after
/// the last of it.
const LOOK_AFTER: Duration = Duration::from_millis(500);

/// A session's badge facts, read when its windows or clients change, when
/// it goes quiet after output (a program started or ended), and when a
/// browser connects. Not a tmux subscription: those cost tmux a format
/// evaluation per session every second, idle or not. An option set by hand
/// (`mouse`, `window-size`) shows at the next of those. The folder is last:
/// it may hold spaces.
const FACTS_FORMAT: &str = "#{session_windows} #{session_attached} #{window-size} #{mouse} #{pane_current_command} #{pane_current_path}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub sessions: Vec<SessionSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    /// tmux's session id, like `$3`.
    pub id: String,
    pub name: String,
    pub panes: Vec<PaneSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSnapshot {
    pub id: PaneId,
    pub cols: u16,
    pub rows: u16,
    /// False until the pane has been seeded from tmux.
    pub synced: bool,
    /// The visible screen as plain text, one line per row.
    pub text: String,
}

/// The running follower. Dropping it stops following without touching tmux
/// beyond detaching its own clients.
pub struct Engine {
    shared: Arc<Shared>,
    supervisor: Mutex<Option<JoinHandle<()>>>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
}

struct Shared {
    backend: Arc<dyn Backend>,
    state: Mutex<State>,
    /// Bumped whenever the set of sessions or panes changes.
    generation: watch::Sender<u64>,
    /// Each rename once, as (old name, new name), whoever made it.
    renamed: broadcast::Sender<(String, String)>,
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<String, Session>,
    panes: HashMap<PaneId, Pane>,
    /// The supervisor has listed the server's sessions once.
    listed: bool,
}

struct Session {
    replace_invalid_utf8: bool,
    name: String,
    panes: Vec<PaneId>,
    requests: mpsc::Sender<Request>,
    /// Who holds the session's sizing seat through us, if anyone.
    seat: Option<u64>,
    facts: Option<Facts>,
    activity: Activity,
}

struct Pane {
    /// The session whose client feeds this pane.
    owner: String,
    screen: vt100::Parser,
    synced: bool,
    /// A capture has been asked for and not yet applied.
    seeding: bool,
    /// The capture waiting for its cursor reply.
    capture: Option<Vec<String>>,
    /// Output after the capture made that capture stale before its cursor.
    late_output: bool,
    /// Updates for whoever is watching this pane. `None` while nobody is.
    viewers: Option<broadcast::Sender<PaneUpdate>>,
    /// Counts changes to the screen, so the overview sends only what moved.
    changes: u64,
    /// A character the output left half done (issue #29).
    utf8: Utf8Gate,
    /// The question the screen asks the operator, if it asks one, as of the
    /// last look at it.
    waiting: Option<String>,
    /// `changes` at that look.
    looked: u64,
}

/// What a viewer of a pane receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneUpdate {
    /// The whole screen, to draw onto a reset terminal of this size. Sent
    /// first, and again whenever the pane is reseeded.
    Screen {
        cols: u16,
        rows: u16,
        bytes: Arc<[u8]>,
    },
    /// Bytes the pane wrote after the last screen.
    Output(Arc<[u8]>),
}

/// A subscription to one pane: the screen as it is now, if the pane is
/// seeded, and every update after it.
pub struct PaneStream {
    pub screen: Option<PaneUpdate>,
    pub updates: broadcast::Receiver<PaneUpdate>,
}

/// How many updates a slow viewer may fall behind before it is sent a fresh
/// screen instead.
const VIEWER_BACKLOG: usize = 256;

/// A pane's visible screen as plain text, one line per row with trailing
/// blanks trimmed, for the overview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub pane: PaneId,
    pub cols: u16,
    pub rows: u16,
    pub text: String,
}

/// What one overview has been sent, for [`Engine::thumbnails`].
#[derive(Debug, Default)]
pub struct Sent {
    /// Each pane's change count when it was last sent, and the round it was
    /// sent in.
    sent: HashMap<PaneId, (u64, u64)>,
    round: u64,
}

impl Sent {
    /// Whether this pane has been sent and is still shown.
    pub fn has(&self, pane: PaneId) -> bool {
        self.sent.contains_key(&pane)
    }

    /// How many panes have been sent and are still shown.
    pub fn len(&self) -> usize {
        self.sent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sent.is_empty()
    }
}

/// A session and its panes, without their contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    /// (pane id, cols, rows)
    pub panes: Vec<(PaneId, u16, u16)>,
    /// Who holds the sizing seat through us, as given to [`Engine::claim`].
    pub seat: Option<u64>,
    /// `None` until tmux has told us.
    pub facts: Option<Facts>,
    pub activity: Activity,
    /// Panes whose screen asks the operator something, and the question
    /// (empty if it isn't on screen). See [`crate::waiting`].
    pub waiting: Vec<(PaneId, String)>,
    /// Whose tmux server it is on, if not the service user's own. Set by
    /// the server hub.
    pub user: Option<String>,
}

/// What makes a session not what it looks like (ADR-0017 decision 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// tmux windows in the session.
    pub windows: u32,
    /// Clients attached other than ours: an SSH user, another tool.
    pub foreign_clients: u32,
    /// The current window's `window-size` is `manual`: its size is fixed.
    pub pinned: bool,
    /// The `mouse` option.
    pub mouse: bool,
    /// What the active pane runs, as tmux names it (`claude`, `bash`).
    pub command: String,
    /// The active pane's folder.
    pub path: String,
}

impl Facts {
    /// Parses a value of [`FACTS_FORMAT`].
    fn parse(value: &str) -> Option<Facts> {
        let mut fields = value.splitn(6, ' ');
        let windows = fields.next()?.parse().ok()?;
        let attached: u32 = fields.next()?.parse().ok()?;
        let pinned = fields.next()? == "manual";
        let mouse = fields.next()? == "1";
        Some(Facts {
            windows,
            // Ours is one of them.
            foreign_clients: attached.saturating_sub(1),
            pinned,
            mouse,
            command: fields.next().unwrap_or_default().to_owned(),
            path: fields.next().unwrap_or_default().to_owned(),
        })
    }
}

/// Whether a session's panes have been writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Activity {
    /// Output in the last few seconds.
    pub active: bool,
    /// When a pane last wrote, if known. Updated when the session goes idle,
    /// not on every write.
    pub last_output: Option<SystemTime>,
}

enum Request {
    Input(PaneId, Vec<u8>),
    Claim { cols: u16, rows: u16 },
    Release,
    RefreshFacts,
    Send(Delivery),
    Stop,
}

enum SupervisorMsg {
    Rediscover,
    Ended(String),
}

impl Engine {
    /// Starts following the sessions on `backend`. Returns immediately;
    /// sessions show up in [`Engine::snapshot`] as they are found.
    pub fn start(backend: impl Backend) -> Engine {
        let (generation, _) = watch::channel(0);
        let shared = Arc::new(Shared {
            backend: Arc::new(backend),
            state: Mutex::new(State::default()),
            generation,
            renamed: broadcast::channel(16).0,
        });
        let (stop, stopped) = oneshot::channel();
        let supervisor = tokio::spawn(supervise(shared.clone(), stopped));
        Engine {
            shared,
            supervisor: Mutex::new(Some(supervisor)),
            stop: Mutex::new(Some(stop)),
        }
    }

    /// Starts a fixed-size session owned by the caller.
    pub async fn create_session(&self, spec: crate::SessionSpec) -> Result<crate::SessionRef> {
        spec.validate()?;
        self.shared.backend.create_session(&spec).await
    }

    /// Kills an owned session only on the server which created it.
    pub async fn kill_session(&self, session: &crate::SessionRef) -> Result<()> {
        self.shared.backend.kill_session(session).await
    }

    /// Check the recorded server generation and immutable ids before
    /// reattaching to an owned session after a node restart.
    pub async fn verify_session(&self, session: &crate::SessionRef) -> Result<()> {
        let reply = self
            .shared
            .backend
            .command(&[
                "display-message",
                "-p",
                "-t",
                &session.session_id,
                "#{session_id} #{pane_id} #{pid} #{start_time}",
            ])
            .await?;
        let current = crate::owned::parse_ref(&reply)?;
        if current != *session {
            bail!("owned seat identity or server generation changed");
        }
        Ok(())
    }

    /// The session named exactly `name` on this server, if any; no server
    /// running means none, but a server it cannot reach is an error. A crashed owner finds the session it created
    /// before it could record its identity.
    pub async fn find_session(&self, name: &str) -> Result<Option<crate::SessionRef>> {
        let target = format!("{name}:");
        let reply = match self
            .shared
            .backend
            .command(&[
                "display-message",
                "-p",
                "-t",
                &target,
                "#{session_name} #{session_id} #{pane_id} #{pid} #{start_time}",
            ])
            .await
        {
            Ok(reply) => reply,
            Err(error) if no_session(&error) || no_server(&error) => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        match reply.trim().split_once(' ') {
            // tmux falls back to a prefix match when no name is exact.
            Some((found, _)) if found != name => Ok(None),
            Some((_, identity)) => crate::owned::parse_ref(identity).map(Some),
            None => bail!("invalid session identity from backend"),
        }
    }

    /// The server of sessions this engine follows.
    pub fn backend(&self) -> &Arc<dyn Backend> {
        &self.shared.backend
    }

    pub fn snapshot(&self) -> Snapshot {
        let state = self.shared.state.lock().unwrap();
        let sessions = state
            .sessions
            .iter()
            .map(|(id, session)| SessionSnapshot {
                id: id.clone(),
                name: session.name.clone(),
                panes: session
                    .panes
                    .iter()
                    .filter_map(|pane_id| {
                        let pane = state.panes.get(pane_id)?;
                        let (rows, cols) = pane.screen.screen().size();
                        Some(PaneSnapshot {
                            id: *pane_id,
                            cols,
                            rows,
                            synced: pane.synced,
                            text: pane.screen.screen().contents(),
                        })
                    })
                    .collect(),
            })
            .collect();
        Snapshot { sessions }
    }

    /// The sessions and their panes' sizes, without screen contents.
    pub fn sessions(&self) -> Vec<SessionInfo> {
        let state = self.shared.state.lock().unwrap();
        state
            .sessions
            .iter()
            .map(|(id, session)| SessionInfo {
                id: id.clone(),
                name: session.name.clone(),
                panes: session
                    .panes
                    .iter()
                    .filter_map(|pane_id| {
                        let (rows, cols) = state.panes.get(pane_id)?.screen.screen().size();
                        Some((*pane_id, cols, rows))
                    })
                    .collect(),
                seat: session.seat,
                facts: session.facts.clone(),
                activity: session.activity,
                waiting: session
                    .panes
                    .iter()
                    .filter_map(|id| Some((*id, state.panes.get(id)?.waiting.clone()?)))
                    .collect(),
                user: None,
            })
            .collect()
    }

    /// The first pane of every session, as plain text, if its screen changed
    /// since `seen` last had it: at most `limit`, those waiting longest
    /// first, so a few busy sessions can't starve the rest and the cost has a
    /// ceiling however many are busy. Updates `seen` and forgets panes that
    /// went. The first pane is the one a tile or Peek shows by default.
    pub fn thumbnails(&self, seen: &mut Sent, limit: usize) -> Vec<Thumbnail> {
        let state = self.shared.state.lock().unwrap();
        let mut shown = Vec::with_capacity(state.sessions.len());
        let mut waiting = Vec::new();
        for session in state.sessions.values() {
            let Some(&id) = session.panes.first() else {
                continue;
            };
            let Some(pane) = state.panes.get(&id).filter(|pane| pane.synced) else {
                continue;
            };
            shown.push(id);
            match seen.sent.get(&id) {
                Some(&(changes, _)) if changes == pane.changes => {}
                sent => waiting.push((sent.map_or(0, |&(_, round)| round), id, pane)),
            }
        }
        seen.sent.retain(|id, _| shown.contains(id));
        waiting.sort_by_key(|&(round, ..)| round);
        seen.round += 1;
        waiting
            .into_iter()
            .take(limit)
            .map(|(_, id, pane)| {
                seen.sent.insert(id, (pane.changes, seen.round));
                let screen = pane.screen.screen();
                let (rows, cols) = screen.size();
                let lines: Vec<String> = screen
                    .rows(0, cols)
                    .map(|row| row.trim_end().to_owned())
                    .collect();
                Thumbnail {
                    pane: id,
                    cols,
                    rows,
                    text: lines.join("\n"),
                }
            })
            .collect()
    }
    /// Starts watching a pane. Nothing is lost or repeated between the screen
    /// returned here and the first update. `None` if the pane doesn't exist.
    pub fn subscribe(&self, pane: PaneId) -> Option<PaneStream> {
        let mut state = self.shared.state.lock().unwrap();
        let pane = state.panes.get_mut(&pane)?;
        let updates = pane
            .viewers
            .get_or_insert_with(|| broadcast::channel(VIEWER_BACKLOG).0)
            .subscribe();
        let screen = pane.synced.then(|| pane.screen_update());
        Some(PaneStream { screen, updates })
    }

    /// A fresh screen for a pane, for a viewer that fell behind.
    pub fn screen(&self, pane: PaneId) -> Option<PaneUpdate> {
        let state = self.shared.state.lock().unwrap();
        let pane = state.panes.get(&pane)?;
        pane.synced.then(|| pane.screen_update())
    }

    /// Reads the visible cells and cursor together from the held screen.
    pub fn read_screen(&self, pane: PaneId) -> Option<crate::Screen> {
        let state = self.shared.state.lock().unwrap();
        let pane = state.panes.get(&pane)?;
        pane.synced
            .then(|| crate::Screen::from_vt(pane.screen.screen()))
    }

    /// A pane's visible screen as plain text, one line per row, once seeded.
    pub fn text(&self, pane: PaneId) -> Option<String> {
        let state = self.shared.state.lock().unwrap();
        let pane = state.panes.get(&pane)?;
        pane.synced.then(|| pane.screen.screen().contents())
    }

    /// Returns once the server's sessions have been listed and each listed
    /// session's panes are known (or it has ended): what [`Engine::sessions`]
    /// says right after [`Engine::start`] is not yet what the server holds.
    pub async fn ready(&self) {
        let mut changed = self.shared.generation.subscribe();
        while !self.shared.state.lock().unwrap().ready() {
            if changed.changed().await.is_err() {
                return;
            }
        }
    }

    /// A receiver that changes whenever sessions or panes come or go.
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.shared.generation.subscribe()
    }

    /// Each session rename on the server, as (old name, new name), whether
    /// Parvi, CHROTE or tmux itself made it.
    pub fn renames(&self) -> broadcast::Receiver<(String, String)> {
        self.shared.renamed.subscribe()
    }

    /// Types bytes into a pane, through the control client that owns it.
    pub async fn send_input(&self, pane: PaneId, data: Vec<u8>) -> Result<()> {
        let requests = {
            let state = self.shared.state.lock().unwrap();
            let owner = &state.panes.get(&pane).context("unknown pane")?.owner;
            state.sessions[owner].requests.clone()
        };
        tell(&requests, Request::Input(pane, data)).await
    }

    /// Queues text for delivery into a pane as one paste, then Enter once the
    /// paste has settled, if asked. The future it returns resolves once tmux
    /// has taken the text and the Enter: delivered, not read (CHROTE 2 PRD).
    /// Dropped before the delivery's turn, it withdraws the delivery.
    pub async fn deliver(&self, pane: PaneId, text: String, enter: bool) -> Result<Delivering> {
        self.queue(Delivery::new(pane, text, enter, None)).await
    }

    /// As [`Engine::deliver`], into a pane that may still be starting: the
    /// paste waits, up to `wait`, until the pane's program has asked for
    /// bracketed paste, its screen is quiet and asks nothing (#242). A pane
    /// that does not get there gets nothing, and the delivery resolves to
    /// [`NotReady`].
    pub async fn deliver_when_ready(
        &self,
        pane: PaneId,
        text: String,
        enter: bool,
        wait: Duration,
    ) -> Result<Delivering> {
        self.queue(Delivery::new(pane, text, enter, Some(wait)))
            .await
    }

    async fn queue(&self, (delivery, delivering): (Delivery, Delivering)) -> Result<Delivering> {
        let requests = {
            let state = self.shared.state.lock().unwrap();
            let owner = &state
                .panes
                .get(&delivery.pane)
                .context("unknown pane")?
                .owner;
            state.sessions[owner].requests.clone()
        };
        tell(&requests, Request::Send(delivery)).await?;
        Ok(delivering)
    }

    /// Takes the sizing seat of the pane's session for `holder`, at this
    /// grid, or follows the grid if `holder` already has it. Our client sizes
    /// the window and the clients that sized it are set to `ignore-size`,
    /// still attached (ADR-0017 decisions 1 and 2). Another holder simply
    /// loses the seat.
    pub async fn claim(&self, pane: PaneId, holder: u64, cols: u16, rows: u16) -> Result<()> {
        // Query the actual pane's window, not the session's selected window:
        // a session can contain several windows with different sizing modes.
        let target = format!("%{pane}");
        let mode = self
            .shared
            .backend
            .command(&["display-message", "-p", "-t", &target, "#{window-size}"])
            .await?;
        if mode.trim() == "manual" {
            bail!("This window has a pinned size and cannot be resized by viewers");
        }
        let requests = {
            let mut state = self.shared.state.lock().unwrap();
            let owner = state
                .panes
                .get(&pane)
                .context("unknown pane")?
                .owner
                .clone();
            let session = state.sessions.get_mut(&owner).context("unknown session")?;
            let changed = session.seat.replace(holder) != Some(holder);
            let requests = session.requests.clone();
            drop(state);
            if changed {
                self.shared.bump();
            }
            requests
        };
        let (cols, rows) = (cols.clamp(1, MAX_GRID), rows.clamp(1, MAX_GRID));
        tell(&requests, Request::Claim { cols, rows }).await
    }

    /// Gives the pane's session's sizing seat back, if `holder` has it: our
    /// client stops sizing and the clients we set aside size again.
    pub async fn release(&self, pane: PaneId, holder: u64) -> Result<()> {
        let requests = {
            let mut state = self.shared.state.lock().unwrap();
            let owner = state
                .panes
                .get(&pane)
                .context("unknown pane")?
                .owner
                .clone();
            let session = state.sessions.get_mut(&owner).context("unknown session")?;
            if session.seat != Some(holder) {
                return Ok(());
            }
            session.seat = None;
            let requests = session.requests.clone();
            drop(state);
            self.shared.bump();
            requests
        };
        tell(&requests, Request::Release).await
    }

    /// Reads every session's badge facts again, for a browser that has just
    /// connected: an option set by hand since shows now.
    pub fn refresh_facts(&self) {
        let state = self.shared.state.lock().unwrap();
        for session in state.sessions.values() {
            let _ = session.requests.try_send(Request::RefreshFacts);
        }
    }

    /// Gives back every seat `holder` has, as when its browser goes.
    pub async fn release_all(&self, holder: u64) {
        let held: Vec<_> = {
            let mut state = self.shared.state.lock().unwrap();
            state
                .sessions
                .values_mut()
                .filter(|session| session.seat == Some(holder))
                .map(|session| {
                    session.seat = None;
                    session.requests.clone()
                })
                .collect()
        };
        if held.is_empty() {
            return;
        }
        self.shared.bump();
        for requests in held {
            let _ = tell(&requests, Request::Release).await;
        }
    }

    /// Gives back every sizing seat, detaches every control client and waits
    /// for them to leave. Works through a shared engine, so browsers still
    /// connected at shutdown don't keep the seats held.
    pub async fn stop(&self) {
        if let Some(stop) = self.stop.lock().unwrap().take() {
            let _ = stop.send(());
        }
        let supervisor = self.supervisor.lock().unwrap().take();
        if let Some(supervisor) = supervisor {
            let _ = supervisor.await;
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(supervisor) = self.supervisor.get_mut().unwrap().take() {
            supervisor.abort();
        }
    }
}

async fn tell(requests: &mpsc::Sender<Request>, request: Request) -> Result<()> {
    requests
        .send(request)
        .await
        .map_err(|_| anyhow!("session is gone"))
}

impl Pane {
    /// Reads the screen for a waiting prompt if it changed since the last
    /// look. True if whether it waits, or what it asks, changed.
    fn look(&mut self) -> bool {
        if self.changes == self.looked {
            return false;
        }
        self.looked = self.changes;
        let screen = self.screen.screen();
        let (_, cols) = screen.size();
        let rows: Vec<String> = screen.rows(0, cols).collect();
        let now = crate::waiting::waiting(rows.iter().map(String::as_str));
        if now == self.waiting {
            return false;
        }
        self.waiting = now;
        true
    }

    fn screen_update(&self) -> PaneUpdate {
        let screen = self.screen.screen();
        let (rows, cols) = screen.size();
        PaneUpdate::Screen {
            cols,
            rows,
            bytes: screen.state_formatted().into(),
        }
    }

    /// Sends an update to the viewers, and forgets them once they've all gone.
    fn tell_viewers(&mut self, update: impl FnOnce(&Pane) -> PaneUpdate) {
        let Some(viewers) = &self.viewers else {
            return;
        };
        if viewers.receiver_count() == 0 {
            self.viewers = None;
            return;
        }
        let _ = viewers.send(update(self));
    }
}

impl State {
    /// See [`Engine::ready`].
    fn ready(&self) -> bool {
        self.listed
            && self.sessions.values().all(|session| {
                !session.panes.is_empty()
                    && session.panes.iter().all(|id| self.panes.contains_key(id))
            })
    }
}

impl Shared {
    fn bump(&self) {
        self.generation.send_modify(|generation| *generation += 1);
    }
}

async fn supervise(shared: Arc<Shared>, mut stopped: oneshot::Receiver<()>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut tasks: HashMap<String, JoinHandle<()>> = HashMap::new();
    loop {
        match list_sessions(shared.backend.as_ref()).await {
            Ok(found) => {
                for (id, name) in found {
                    if tasks.contains_key(&id) {
                        continue;
                    }
                    let (requests, requests_rx) = mpsc::channel(64);
                    shared.state.lock().unwrap().sessions.insert(
                        id.clone(),
                        Session {
                            replace_invalid_utf8: false,
                            name,
                            panes: Vec::new(),
                            requests,
                            seat: None,
                            facts: None,
                            activity: Activity::default(),
                        },
                    );
                    shared.bump();
                    let task =
                        tokio::spawn(follow(shared.clone(), id.clone(), requests_rx, tx.clone()));
                    tasks.insert(id, task);
                }
            }
            Err(error) => eprintln!("parvi: listing sessions: {error:#}"),
        }
        let first = !std::mem::replace(&mut shared.state.lock().unwrap().listed, true);
        if first {
            shared.bump();
        }

        // Wait for something to change. With no sessions there is no client to
        // hear about new ones, so look again after a moment.
        let msg = tokio::select! {
            _ = &mut stopped => break,
            msg = rx.recv() => msg,
            _ = tokio::time::sleep(NO_SERVER_RETRY), if tasks.is_empty() => None,
        };
        let mut msgs: Vec<SupervisorMsg> = msg.into_iter().collect();
        while let Ok(msg) = rx.try_recv() {
            msgs.push(msg);
        }
        for msg in msgs {
            if let SupervisorMsg::Ended(id) = msg {
                tasks.remove(&id);
                forget_session(&shared, &id);
            }
        }
    }

    // Stop: ask every client to detach, then wait for them.
    let requests: Vec<_> = shared
        .state
        .lock()
        .unwrap()
        .sessions
        .values()
        .map(|session| session.requests.clone())
        .collect();
    for request in requests {
        let _ = request.send(Request::Stop).await;
    }
    for (_, task) in tasks {
        let _ = task.await;
    }
}

fn forget_session(shared: &Shared, id: &str) {
    let mut state = shared.state.lock().unwrap();
    state.sessions.remove(id);
    state.panes.retain(|_, pane| pane.owner != id);
    drop(state);
    shared.bump();
}

/// Lists `(session id, name)` on the backend. No server means no sessions.
async fn list_sessions(backend: &dyn Backend) -> Result<Vec<(String, String)>> {
    let result = backend
        .command(&["list-sessions", "-F", "#{session_id} #{session_name}"])
        .await;
    let output = match result {
        Ok(output) => output,
        Err(error) if no_server(&error) => return Ok(Vec::new()),
        Err(error) => return Err(error.context("list-sessions")),
    };
    Ok(output
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(id, name)| (id.to_owned(), name.to_owned()))
        .collect())
}

/// Whether the backend failed because no server is running: tmux exits with
/// its last session, so that is an empty server, not a fault. tmux says "no
/// server running on PATH" when a stale socket file refuses the connection
/// and "error connecting to PATH (No such file or directory)" when there is
/// none, and "[server exited unexpectedly]" when the server accepted the
/// command but exited before answering, as one that `kill-server` just told
/// to go does (#232): it is gone, with its sessions. Any other connection
/// error, such as a socket it may not open, is a fault: a server may be there. The socket path is the operator's to name,
/// so only the words around it are read.
fn no_server(error: &anyhow::Error) -> bool {
    let error = error.to_string();
    let error = error.trim();
    error.trim_matches(['[', ']']) == "server exited unexpectedly"
        || error.starts_with("no server running on ")
        || (error.starts_with("error connecting to ")
            && error.ends_with(" (No such file or directory)"))
}

/// Whether the backend found the server but not the session. tmux and the
/// holder both say "can't find session: NAME"; a connection error names the
/// socket path, which may hold those words, so only the start is read.
fn no_session(error: &anyhow::Error) -> bool {
    error.to_string().trim().starts_with("can't find session: ")
}

/// What a command we wrote was, so its reply can be handled.
enum Pending {
    Version,
    ListPanes,
    Capture(PaneId),
    Cursor(PaneId),
    Input,
    Continue,
    Me,
    Clients,
    Observers,
    Seat,
    Delivery(delivery::Step),
    /// The current window's last activity, to start from before any output.
    Activity,
    Facts,
}

const LIST_PANES_FORMAT: &str = "#{pane_id} #{pane_width} #{pane_height}";

/// Follows one session until its client exits or the engine stops.
async fn follow(
    shared: Arc<Shared>,
    session: String,
    mut requests: mpsc::Receiver<Request>,
    supervisor: mpsc::UnboundedSender<SupervisorMsg>,
) {
    if let Err(error) = follow_inner(&shared, &session, &mut requests, &supervisor).await {
        eprintln!("parvi: following {session}: {error:#}");
    }
    let _ = supervisor.send(SupervisorMsg::Ended(session));
}

async fn follow_inner(
    shared: &Shared,
    session: &str,
    requests: &mut mpsc::Receiver<Request>,
    supervisor: &mpsc::UnboundedSender<SupervisorMsg>,
) -> Result<()> {
    let mut client = shared.backend.attach(session).await?;
    // Kept across loop turns: `read_until` keeps a partly read line here when
    // another branch of the select wins.
    let mut line = Vec::new();
    let mut parser = std::mem::take(&mut client.parser);
    let mut pending = VecDeque::new();

    let mut seat = Seat::default();
    let mut deliveries = Deliveries::default();
    // When Enter is due for the delivery whose paste is settling.
    let settle = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(settle);
    // When to look whether the pane a delivery waits on is ready, as the
    // timer is set; `None` while no delivery waits.
    let ready = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(ready);
    let mut ready_at = None;
    // When one of our panes last wrote, and the moment to look again whether
    // the session has gone quiet. Shared state changes only when it does.
    let mut active = false;
    let mut last_output = tokio::time::Instant::now();
    // tmux reports a layout change before an interactive shell redraws its
    // existing prompt in response to the resize.
    let mut layout_changed: Option<tokio::time::Instant> = None;
    let quiet = tokio::time::sleep(ACTIVE_FOR);
    tokio::pin!(quiet);
    // A look at our panes' screens for a waiting prompt is due.
    let mut looking = false;
    let look = tokio::time::sleep(LOOK_AFTER);
    tokio::pin!(look);

    let mut out = String::new();
    send(
        &mut out,
        &mut pending,
        "display-message -p '#{version}'".to_owned(),
        Pending::Version,
    );
    send(
        &mut out,
        &mut pending,
        "display-message -p '#{client_name}'".to_owned(),
        Pending::Me,
    );
    send(
        &mut out,
        &mut pending,
        list_panes(session),
        Pending::ListPanes,
    );
    request_facts(session, &mut out, &mut pending);
    send(
        &mut out,
        &mut pending,
        "display-message -p '#{window_activity}'".to_owned(),
        Pending::Activity,
    );

    loop {
        if !out.is_empty() {
            client.writer.write_all(out.as_bytes()).await?;
            client.writer.flush().await?;
            out.clear();
        }
        let look_at = deliveries.readying().map(|(_, at)| at);
        if look_at != ready_at {
            if let Some(at) = look_at {
                ready.as_mut().reset(at);
            }
            ready_at = look_at;
        }
        tokio::select! {
            read = client.reader.read_until(b'\n', &mut line) => {
                if read? == 0 {
                    break;
                }
                let event = parser.line(line.strip_suffix(b"\n").unwrap_or(&line));
                line.clear();
                let Some(event) = event else { continue };
                match event {
                    Event::Reply { from_us: false, .. } => {}
                    Event::Reply { from_us: true, ok, lines } => {
                        let what = pending.pop_front().context("reply to nothing")?;
                        if !ok {
                            eprintln!("parvi: {session}: tmux error: {}", lines.join(" "));
                        }
                        match what {
                            Pending::Version => {
                                if let Some(entry) = shared.state.lock().unwrap().sessions.get_mut(session) {
                                    entry.replace_invalid_utf8 = ok && lines.first().is_some_and(|v| crate::utf8::tmux_replaces_invalid(v));
                                }
                            }
                            Pending::Me => {
                                seat.me = lines.into_iter().next().filter(|_| ok);
                                // Attached now, so we'll hear of sessions from
                                // here on. One made since the supervisor last
                                // listed would otherwise go unseen.
                                let _ = supervisor.send(SupervisorMsg::Rediscover);
                            }
                            Pending::Clients => seat.listed(ok, &lines, &mut out, &mut pending),
                            Pending::Delivery(step) => {
                                deliveries.replied(session, step, ok, &lines, &mut out, &mut pending);
                                if let Some(due) = deliveries.due() {
                                    settle.as_mut().reset(due);
                                }
                            }
                            what => on_reply(shared, session, what, ok, lines, &mut out, &mut pending),
                        }
                    }
                    Event::Notification(Notification::Output { pane, data }) => {
                        deliveries.heard_output(pane, tokio::time::Instant::now());
                        let mut state = shared.state.lock().unwrap();
                        let Some(pane) = state.panes.get_mut(&pane) else { continue };
                        if pane.owner != session {
                            continue;
                        }
                        let possible_redraw = pane.synced
                            && layout_changed.is_some_and(|at| at.elapsed() < Duration::from_secs(2))
                            && data.starts_with(b"\r\x1b[K\r");
                        let before = possible_redraw.then(|| pane.screen.screen().contents());
                        let mut unchanged_redraw = false;
                        if pane.synced {
                            let data = pane.utf8.pass(data);
                            pane.screen.process(&data);
                            unchanged_redraw = before.is_some_and(|text| text == pane.screen.screen().contents());
                            pane.changes += 1;
                            pane.tell_viewers(|_| PaneUpdate::Output(data.into()));
                        } else if pane.capture.is_some() {
                            // This output is later than the captured cells and
                            // cannot be discarded when that capture is applied.
                            pane.late_output = true;
                        }
                        if unchanged_redraw {
                            continue;
                        }
                        last_output = tokio::time::Instant::now();
                        if !looking {
                            looking = true;
                            look.as_mut().reset(last_output + LOOK_AFTER);
                        }
                        if !active {
                            active = true;
                            quiet.as_mut().reset(last_output + ACTIVE_FOR);
                            if let Some(entry) = state.sessions.get_mut(session) {
                                entry.activity = Activity { active: true, last_output: Some(SystemTime::now()) };
                            }
                            drop(state);
                            shared.bump();
                        }
                    }
                    Event::Notification(Notification::ClientsChanged) => {
                        request_facts(session, &mut out, &mut pending);
                    }
                    Event::Notification(Notification::Pause { pane: id }) => {
                        // We fell behind and tmux dropped the pane's output
                        // for us. Continue it at once, even a pane another
                        // session feeds, which may become ours later. Ours
                        // starts over from a capture, like first sync.
                        send(&mut out, &mut pending, format!("refresh-client -A '%{id}:continue'"), Pending::Continue);
                        let mut state = shared.state.lock().unwrap();
                        if let Some(pane) = state.panes.get_mut(&id)
                            && pane.owner == session {
                                reseed(pane, id, &mut out, &mut pending);
                                drop(state);
                                shared.bump();
                            }
                    }
                    Event::Notification(Notification::WindowsChanged) => {
                        layout_changed = Some(tokio::time::Instant::now());
                        send(&mut out, &mut pending, list_panes(session), Pending::ListPanes);
                        request_facts(session, &mut out, &mut pending);
                    }
                    Event::Notification(Notification::SessionsChanged) => {
                        let _ = supervisor.send(SupervisorMsg::Rediscover);
                    }
                    Event::Notification(Notification::SessionRenamed { session: renamed, name }) => {
                        // Any session's rename, not only this client's.
                        // Every control client hears it; the first tells.
                        if let Some(entry) = shared.state.lock().unwrap().sessions.get_mut(&renamed)
                            && entry.name != name
                        {
                            let old = std::mem::replace(&mut entry.name, name.clone());
                            let _ = shared.renamed.send((old, name));
                        }
                        shared.bump();
                    }
                    Event::Notification(Notification::Exit { .. }) => break,
                    Event::Notification(Notification::Other { .. }) => {}
                }
            }
            () = &mut quiet, if active => {
                let due = last_output + ACTIVE_FOR;
                if due > tokio::time::Instant::now() {
                    quiet.as_mut().reset(due);
                } else {
                    active = false;
                    let wrote = SystemTime::now() - last_output.elapsed();
                    if let Some(entry) = shared.state.lock().unwrap().sessions.get_mut(session) {
                        entry.activity = Activity { active: false, last_output: Some(wrote) };
                    }
                    shared.bump();
                    // A program that started or ended wrote first.
                    request_facts(session, &mut out, &mut pending);
                }
            }
            () = &mut look, if looking => {
                looking = false;
                let mut state = shared.state.lock().unwrap();
                let mut changed = false;
                for pane in state.panes.values_mut().filter(|pane| pane.owner == session) {
                    changed |= pane.look();
                }
                drop(state);
                if changed {
                    shared.bump();
                }
            }
            () = &mut settle, if deliveries.due().is_some() => {
                deliveries.settled(&mut out, &mut pending);
            }
            () = &mut ready, if ready_at.is_some() => {
                // Set again at the top of the loop, from the next look.
                ready_at = None;
                if let Some((pane, _)) = deliveries.readying() {
                    let readiness = readiness(shared, pane);
                    deliveries.looked(session, readiness, tokio::time::Instant::now(), &mut out, &mut pending);
                }
            }
            request = requests.recv() => match request {
                Some(Request::Input(pane, data)) => {
                    send(&mut out, &mut pending, control::send_keys_hex(pane, &data), Pending::Input);
                }
                Some(Request::Claim { cols, rows }) => {
                    seat.claim(session, cols, rows, &mut out, &mut pending);
                }
                Some(Request::Release) => seat.release(&mut out, &mut pending),
                Some(Request::RefreshFacts) => request_facts(session, &mut out, &mut pending),
                Some(Request::Send(delivery)) => {
                    deliveries.push(session, delivery, &mut out, &mut pending);
                }
                Some(Request::Stop) | None => {
                    // Leaving while holding the seat would leave the clients
                    // we set aside unable to size the window.
                    seat.release(&mut out, &mut pending);
                    client.writer.write_all(out.as_bytes()).await?;
                    client.writer.flush().await?;
                    break;
                }
            },
        }
    }

    client.close().await
}

/// One session's sizing seat, as our client sees it.
#[derive(Default)]
struct Seat {
    /// Our client's name, once tmux has said it.
    me: Option<String>,
    /// The grid a browser wants, or `None` while nobody claims.
    wanted: Option<(u16, u16)>,
    /// A `list-clients` for a claim is on its way.
    listing: bool,
    /// Our client is sizing the window.
    held: bool,
    /// The clients that were sizing the window when we claimed it, which we
    /// set to `ignore-size`.
    set_aside: Vec<String>,
}

impl Seat {
    fn claim(
        &mut self,
        session: &str,
        cols: u16,
        rows: u16,
        out: &mut String,
        pending: &mut VecDeque<Pending>,
    ) {
        self.wanted = Some((cols, rows));
        if self.held {
            send(
                out,
                pending,
                format!("refresh-client -C {cols},{rows}"),
                Pending::Seat,
            );
        } else if !self.listing {
            self.listing = true;
            send(
                out,
                pending,
                format!("list-clients -t '{session}' -F '#{{client_name}} #{{client_flags}}'"),
                Pending::Clients,
            );
        }
    }

    /// The session's clients have been listed for a claim: take the seat, if
    /// it is still wanted.
    fn listed(
        &mut self,
        ok: bool,
        lines: &[String],
        out: &mut String,
        pending: &mut VecDeque<Pending>,
    ) {
        self.listing = false;
        let (Some((cols, rows)), Some(me), false, true) = (self.wanted, &self.me, self.held, ok)
        else {
            return;
        };
        for line in lines {
            let Some((name, flags)) = line.split_once(' ') else {
                continue;
            };
            if name == me || flags.split(',').any(|flag| flag == "ignore-size") {
                continue;
            }
            send(
                out,
                pending,
                format!("refresh-client -t '{name}' -f ignore-size"),
                Pending::Seat,
            );
            self.set_aside.push(name.to_owned());
        }
        send(
            out,
            pending,
            "refresh-client -f '!ignore-size'".to_owned(),
            Pending::Seat,
        );
        send(
            out,
            pending,
            format!("refresh-client -C {cols},{rows}"),
            Pending::Seat,
        );
        self.held = true;
    }

    /// Stops sizing, and lets the clients we set aside size again. Ours goes
    /// first, so the window takes their size rather than ours.
    fn release(&mut self, out: &mut String, pending: &mut VecDeque<Pending>) {
        let wanted = self.wanted.take();
        if !self.held {
            return;
        }
        self.held = false;
        let (cols, rows) = wanted.unwrap_or((1, 1));
        send(
            out,
            pending,
            "refresh-client -f ignore-size".to_owned(),
            Pending::Seat,
        );
        for name in self.set_aside.drain(..) {
            send(
                out,
                pending,
                format!("refresh-client -t '{name}' -f '!ignore-size'"),
                Pending::Seat,
            );
        }
        // Changing flags doesn't make tmux size the window again; that waits
        // for its next resize check, seconds later on tmux 3.4. A size from
        // our client, ignored now, makes tmux do it at once.
        send(
            out,
            pending,
            format!("refresh-client -C {cols},{rows}"),
            Pending::Seat,
        );
    }
}

/// Whether `pane` is ready for a paste a delivery holds for it (#242).
fn readiness(shared: &Shared, pane: PaneId) -> Readiness {
    let mut state = shared.state.lock().unwrap();
    let Some(pane) = state.panes.get_mut(&pane) else {
        return Readiness::Gone;
    };
    if !pane.synced {
        return Readiness::NotYet;
    }
    // Read the screen now rather than at the next look after output.
    let changed = pane.look();
    let readiness = if pane.waiting.is_some() {
        Readiness::Asking
    } else if pane.screen.screen().bracketed_paste() {
        Readiness::Ready
    } else {
        Readiness::NotYet
    };
    drop(state);
    if changed {
        shared.bump();
    }
    readiness
}

fn facts_command() -> String {
    format!("display-message -p '{FACTS_FORMAT}'")
}

/// A second Parvi node follows in control mode without owning the sizing
/// seat. Count actual attached viewers separately from those followers.
fn request_facts(session: &str, out: &mut String, pending: &mut VecDeque<Pending>) {
    send(out, pending, facts_command(), Pending::Facts);
    send(
        out,
        pending,
        format!("list-clients -t '{session}' -F '#{{client_flags}}'"),
        Pending::Observers,
    );
}

fn list_panes(session: &str) -> String {
    format!("list-panes -s -t '{session}' -F '{LIST_PANES_FORMAT}'")
}

/// Starts a pane over from tmux's copy: asks for its screen and cursor, and
/// drops output before the capture reply, which already holds it. Output
/// after that reply invalidates the capture, so the cursor reply retries it.
/// The capture joins the rows tmux wrapped (`-J`), so the seeded screen wraps
/// them again and they stay one line to a viewer (#186).
fn reseed(pane: &mut Pane, id: PaneId, out: &mut String, pending: &mut VecDeque<Pending>) {
    pane.synced = false;
    pane.utf8.reset();
    pane.seeding = true;
    pane.capture = None;
    pane.late_output = false;
    send(
        out,
        pending,
        format!("capture-pane -p -e -J -t %{id}"),
        Pending::Capture(id),
    );
    send(
        out,
        pending,
        format!(
            "display-message -p -t %{id} '#{{cursor_x}} #{{cursor_y}} #{{alternate_on}} #{{cursor_flag}}'"
        ),
        Pending::Cursor(id),
    );
}

fn send(out: &mut String, pending: &mut VecDeque<Pending>, command: String, what: Pending) {
    out.push_str(&command);
    out.push('\n');
    pending.push_back(what);
}

fn on_reply(
    shared: &Shared,
    session: &str,
    what: Pending,
    ok: bool,
    lines: Vec<String>,
    out: &mut String,
    pending: &mut VecDeque<Pending>,
) {
    let mut state = shared.state.lock().unwrap();
    let replace_invalid_utf8 = state
        .sessions
        .get(session)
        .is_some_and(|s| s.replace_invalid_utf8);
    match what {
        Pending::Input | Pending::Continue | Pending::Seat => {}
        Pending::Facts => {
            let mut facts = lines
                .first()
                .filter(|_| ok)
                .and_then(|line| Facts::parse(line));
            if let Some(entry) = state.sessions.get_mut(session)
                && let Some(facts) = &mut facts
            {
                // Stay conservative until the corresponding client list has
                // arrived; a later facts poll must retain the counted value.
                facts.foreign_clients = entry.facts.as_ref().map_or(1, |f| f.foreign_clients);
            }
            if let Some(entry) = state.sessions.get_mut(session)
                && facts.is_some()
                && entry.facts != facts
            {
                entry.facts = facts;
                drop(state);
                shared.bump();
            }
        }
        Pending::Observers => {
            if ok {
                let count = lines
                    .iter()
                    .filter(|line| {
                        let mut flags = line.split(',');
                        !(flags.clone().any(|f| f == "control-mode")
                            && flags.any(|f| f.starts_with("pause-after")))
                    })
                    .count() as u32;
                if let Some(facts) = state
                    .sessions
                    .get_mut(session)
                    .and_then(|s| s.facts.as_mut())
                    && facts.foreign_clients != count
                {
                    facts.foreign_clients = count;
                    drop(state);
                    shared.bump();
                }
            }
        }
        Pending::Activity => {
            let seconds = lines.first().and_then(|line| line.parse::<u64>().ok());
            if let (true, Some(seconds), Some(entry)) =
                (ok, seconds, state.sessions.get_mut(session))
                && entry.activity.last_output.is_none()
            {
                entry.activity.last_output =
                    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds));
                drop(state);
                shared.bump();
            }
        }
        Pending::Version | Pending::Me | Pending::Clients | Pending::Delivery(_) => {
            unreachable!("handled in follow_inner")
        }
        Pending::ListPanes if ok => {
            let listed: Vec<(PaneId, u16, u16)> = lines
                .iter()
                .filter_map(|line| {
                    let mut fields = line.split(' ');
                    let id = control::parse_pane(fields.next()?)?;
                    let cols = fields.next()?.parse().ok()?;
                    let rows = fields.next()?.parse().ok()?;
                    Some((id, cols, rows))
                })
                .collect();
            let mut changed = false;
            for &(id, cols, rows) in &listed {
                let pane = state.panes.entry(id).or_insert_with(|| {
                    changed = true;
                    Pane {
                        owner: session.to_owned(),
                        screen: pane_screen(rows, cols, replace_invalid_utf8),
                        synced: false,
                        seeding: false,
                        capture: None,
                        late_output: false,
                        viewers: None,
                        changes: 0,
                        utf8: Utf8Gate::new(replace_invalid_utf8),
                        waiting: None,
                        looked: 0,
                    }
                });
                if pane.owner != session {
                    continue;
                }
                let size = pane.screen.screen().size();
                // A new pane needs seeding. A resized one does too, because
                // tmux reflows the pane itself and sends no output for that.
                if (!pane.synced && !pane.seeding) || size != (rows, cols) {
                    pane.screen.screen_mut().set_size(rows, cols);
                    reseed(pane, id, out, pending);
                    changed = true;
                }
            }
            let listed_ids: Vec<PaneId> = listed.iter().map(|&(id, _, _)| id).collect();
            let before = state.panes.len();
            state
                .panes
                .retain(|id, pane| pane.owner != session || listed_ids.contains(id));
            changed |= state.panes.len() != before;
            if let Some(entry) = state.sessions.get_mut(session)
                && entry.panes != listed_ids
            {
                entry.panes = listed_ids;
                changed = true;
            }
            drop(state);
            if changed {
                shared.bump();
            }
        }
        Pending::ListPanes => {}
        Pending::Capture(id) => {
            if let Some(pane) = state.panes.get_mut(&id) {
                pane.capture = Some(if ok { lines } else { Vec::new() });
            }
        }
        Pending::Cursor(id) => {
            let Some(pane) = state.panes.get_mut(&id) else {
                return;
            };
            if pane.late_output {
                // The output is not in the capture, and the cursor reply may
                // already include it. Retry rather than replaying it at the
                // wrong cursor position; only publish a coherent snapshot.
                reseed(pane, id, out, pending);
                return;
            }
            let Some(capture) = pane.capture.take() else {
                return;
            };
            let mut fields = lines.first().map(String::as_str).unwrap_or("").split(' ');
            let x: u16 = fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);
            let y: u16 = fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);
            let alternate = fields.next() == Some("1");
            let visible = fields.next() != Some("0");
            let (rows, cols) = pane.screen.screen().size();
            // A capture holds no input modes. Bracketed paste is kept from
            // the screen this one replaces: a delivery waits for it (#242).
            let bracketed = pane.screen.screen().bracketed_paste();
            pane.screen = pane_screen(rows, cols, replace_invalid_utf8);
            pane.screen
                .process(&seed(&capture, x, y, alternate, visible));
            if bracketed {
                pane.screen.process(b"\x1b[?2004h");
            }
            let rendition = crate::rendition::before_cursor(pane.screen.screen(), y, x);
            pane.screen.process(&rendition);
            pane.synced = true;
            pane.seeding = false;
            pane.changes += 1;
            // A prompt may already be up, with no output to come.
            pane.look();
            pane.tell_viewers(Pane::screen_update);
            drop(state);
            shared.bump();
        }
    }
}

fn pane_screen(rows: u16, cols: u16, replace_invalid_utf8: bool) -> vt100::Parser {
    let mut screen = vt100::Parser::new(rows, cols, SCROLLBACK_LINES);
    if replace_invalid_utf8 {
        screen.render_replacement();
    }
    screen
}

/// The bytes that redraw a captured screen into a fresh emulator: the lines,
/// then the cursor where tmux has it.
fn seed(capture: &[String], x: u16, y: u16, alternate: bool, visible: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if alternate {
        bytes.extend_from_slice(b"\x1b[?1049h");
    }
    bytes.extend_from_slice(capture.join("\r\n").as_bytes());
    bytes.extend_from_slice(format!("\x1b[0m\x1b[{};{}H", y + 1, x + 1).as_bytes());
    if !visible {
        bytes.extend_from_slice(b"\x1b[?25l");
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_read_the_command_and_a_folder_with_spaces() {
        let f = Facts::parse("2 3 manual 1 claude /srv/my app").unwrap();
        assert_eq!(
            (f.windows, f.foreign_clients, f.pinned, f.mouse),
            (2, 2, true, true)
        );
        assert_eq!(
            (f.command.as_str(), f.path.as_str()),
            ("claude", "/srv/my app")
        );
        let older = Facts::parse("1 1 latest 0").unwrap();
        assert_eq!((older.command.as_str(), older.path.as_str()), ("", ""));
    }

    #[test]
    fn seeding_redraws_the_capture_and_places_the_cursor() {
        let capture = vec!["\x1b[1mbold\x1b[0m line".to_owned(), "$ ".to_owned()];
        let mut screen = vt100::Parser::new(5, 20, 0);
        screen.process(&seed(&capture, 2, 1, false, false));
        assert_eq!(screen.screen().contents(), "bold line\n$ ");
        assert_eq!(screen.screen().cursor_position(), (1, 2));
        assert!(screen.screen().cell(0, 0).unwrap().bold());
        assert!(screen.screen().hide_cursor());
    }

    #[test]
    fn seeding_an_alternate_screen_switches_to_it() {
        let mut screen = vt100::Parser::new(5, 20, 0);
        screen.process(&seed(&["vim".to_owned()], 0, 0, true, true));
        assert!(screen.screen().alternate_screen());
    }

    /// tmux 3.6a's words for each way a connection fails.
    #[test]
    fn only_an_absent_or_stale_socket_means_no_server() {
        let says = |text: &str| no_server(&anyhow::anyhow!("{text}"));
        assert!(says("no server running on /tmp/s/tmux"));
        assert!(says("[server exited unexpectedly]"));
        assert!(says("server exited unexpectedly"));
        assert!(says(
            "error connecting to /tmp/s/tmux (No such file or directory)"
        ));
        assert!(!says("error connecting to /tmp/s/tmux (Permission denied)"));
        assert!(!says(
            "error connecting to /tmp/s/tmux (Resource temporarily unavailable)"
        ));
        assert!(!says("BLOCKED by CHROTE tmux wrapper: kill-server"));
        // The path may hold the very words, and is not the reason.
        assert!(!says(
            "error connecting to /tmp/no server running on/tmux (Permission denied)"
        ));
        assert!(!says(
            "error connecting to /tmp/error connecting to (No such file or directory)/tmux (Permission denied)"
        ));
    }

    #[test]
    fn only_tmux_saying_so_means_no_session() {
        let says = |text: &str| no_session(&anyhow::anyhow!("{text}"));
        assert!(says("can't find session: owned"));
        assert!(!says(
            "error connecting to /tmp/can't find session: owned/tmux (Permission denied)"
        ));
    }
}
