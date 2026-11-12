//! One client's terminals on the node's socket: the session list on connect
//! and whenever it changes, the launcher, and the panes it subscribes to: a
//! full screen first, then output as it happens. A subscription is a task
//! forwarding one pane's updates; dropping it costs the engine nothing. The
//! overview is one more such task, sending every session's first pane as
//! text when it changed, at most twice a second. A client resizes tmux only
//! by claiming a session's sizing seat, and gives every seat back when it
//! goes. Launching and deleting sessions run in tasks of their own and
//! answer with a done frame. A client has at most one search running; a new
//! one, an empty one, or leaving stops it. Everything goes through the
//! [`Hub`], so a client sees every tmux server the node follows as one.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parvi_node_kit::{BoxFuture, Channel, Client};
use parvi_term::control::PaneId;
use parvi_term::engine::PaneUpdate;
use parvi_term::search::Target;
use parvi_terminals_contract::{self as contract, Up};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::frames;
use crate::hub::{Hub, HubSent};
use crate::launch::{self, Config};
use crate::launcher::Launcher;

/// Frames queued for one client before it counts as stuck.
const OUTBOX: usize = 1024;

/// How often the overview looks for changed panes.
const OVERVIEW_EVERY: Duration = Duration::from_millis(500);

/// The most thumbnails one look sends: 40 a second. Below that every changed
/// pane is sent each look; above it, those waiting longest go first.
const OVERVIEW_BATCH: usize = 20;

pub(crate) struct Terminal {
    me: u64,
    hub: Arc<Hub>,
    launcher: Arc<Launcher>,
    outbox: mpsc::Sender<Vec<u8>>,
    outgoing: mpsc::Receiver<Vec<u8>>,
    changes: watch::Receiver<u64>,
    launcher_changes: watch::Receiver<Arc<Config>>,
    subscriptions: HashMap<PaneId, JoinHandle<()>>,
    overview: Option<JoinHandle<()>>,
    searching: Option<JoinHandle<()>>,
    /// The client looks at the session list while it is connected.
    _looking: Vec<parvi_term::engine::Looking>,
}

impl Terminal {
    /// A client's terminals, with the session list and the launcher queued
    /// for it first.
    pub(crate) fn new(client: Client, hub: Arc<Hub>, launcher: Arc<Launcher>) -> Terminal {
        let (outbox, outgoing) = mpsc::channel(OUTBOX);
        launcher.look();
        let mut launcher_changes = launcher.watch();
        let changes = hub.watch();
        let looking = hub.look();
        hub.refresh_facts();
        let first = frames::launcher(
            &launcher_changes.borrow_and_update(),
            &hub.users(),
            hub.notify_off(),
        );
        let _ = outbox.try_send(sessions(&hub, client.id));
        let _ = outbox.try_send(first);
        Terminal {
            me: client.id,
            hub,
            launcher,
            outbox,
            outgoing,
            changes,
            launcher_changes,
            subscriptions: HashMap::new(),
            overview: None,
            searching: None,
            _looking: looking,
        }
    }

    async fn handle(&mut self, up: Up) {
        let (hub, outbox, me) = (&self.hub, &self.outbox, self.me);
        match up {
            Up::Subscribe(pane) => {
                if let Some(old) = self.subscriptions.remove(&pane) {
                    old.abort();
                }
                let task = tokio::spawn(forward(hub.clone(), pane, outbox.clone()));
                self.subscriptions.insert(pane, task);
            }
            Up::Unsubscribe(pane) => {
                if let Some(task) = self.subscriptions.remove(&pane) {
                    task.abort();
                }
            }
            Up::Input(pane, data) => {
                if let Err(error) = hub.send_input(pane, data).await {
                    eprintln!("parvi: input to %{pane}: {error:#}");
                }
            }
            Up::Claim(pane, cols, rows) => {
                if let Err(error) = hub.claim(pane, me, cols, rows).await {
                    let refused = contract::claim_refused(pane, &format!("{error:#}"));
                    let _ = outbox.send(refused).await;
                }
            }
            Up::Release(pane) => {
                if let Err(error) = hub.release(pane, me).await {
                    eprintln!("parvi: release for %{pane}: {error:#}");
                }
            }
            Up::Send {
                pane,
                id,
                enter,
                ready,
                text,
            } => {
                // Queued here, in order with input; tmux's answer is
                // awaited in a task so it holds up nothing else.
                let queued = if ready {
                    hub.deliver_when_ready(pane, text, enter).await
                } else {
                    hub.deliver(pane, text, enter).await
                };
                let outbox = outbox.clone();
                tokio::spawn(async move {
                    let delivered = match queued {
                        Ok(delivery) => delivery.await,
                        Err(error) => Err(error),
                    };
                    if let Err(error) = &delivered {
                        eprintln!("parvi: send to %{pane}: {error:#}");
                    }
                    let outcome = frames::outcome(&delivered);
                    let _ = outbox.send(contract::delivered(pane, id, outcome)).await;
                });
            }
            Up::Overview(on) => {
                if let Some(old) = self.overview.take() {
                    old.abort();
                }
                if on {
                    self.overview = Some(tokio::spawn(thumbnails(hub.clone(), outbox.clone())));
                }
            }
            Up::Launch { id, launch } => {
                let (hub, launcher, outbox) = (hub.clone(), self.launcher.get(), outbox.clone());
                tokio::spawn(async move {
                    let frame = match hub.launch(&launcher, &launch).await {
                        Ok((session, None)) => contract::done(id, true, &session),
                        Ok((session, Some(warning))) => {
                            eprintln!("parvi: launching {}: {warning}", launch.name);
                            contract::done(id, true, &format!("{session}\0{warning}"))
                        }
                        Err(error) => {
                            eprintln!("parvi: launching {}: {error:#}", launch.name);
                            contract::done(id, false, &format!("{error:#}"))
                        }
                    };
                    let _ = outbox.send(frame).await;
                });
            }
            Up::Delete { id, session } => {
                let (hub, outbox) = (hub.clone(), outbox.clone());
                tokio::spawn(async move {
                    let deleted = match hub.session(&session) {
                        Ok((server, id)) => launch::delete(server.backend(), id).await,
                        Err(error) => Err(error),
                    };
                    let frame = match deleted {
                        Ok(()) => contract::done(id, true, &session),
                        Err(error) => contract::done(id, false, &format!("{error:#}")),
                    };
                    let _ = outbox.send(frame).await;
                });
            }
            Up::Rename { id, session, name } => {
                let (hub, outbox) = (hub.clone(), outbox.clone());
                tokio::spawn(async move {
                    let frame = match hub.rename(&session, &name).await {
                        Ok(()) => contract::done(id, true, &session),
                        Err(error) => contract::done(id, false, &format!("{error:#}")),
                    };
                    let _ = outbox.send(frame).await;
                });
            }
            Up::Mouse { id, on } => {
                let (hub, outbox) = (hub.clone(), outbox.clone());
                tokio::spawn(async move {
                    let mouse = hub.mouse(on).await;
                    let ok = mouse.failures.is_empty();
                    let _ = outbox.send(contract::done(id, ok, &mouse.text(on))).await;
                });
            }
            Up::Search { id, query } => {
                if let Some(old) = self.searching.take() {
                    old.abort();
                }
                if !query.is_empty() {
                    let task = search(hub.clone(), id, query, outbox.clone());
                    self.searching = Some(tokio::spawn(task));
                }
            }
            Up::History { pane, id } => {
                let (hub, outbox) = (hub.clone(), outbox.clone());
                tokio::spawn(async move {
                    let capture = match hub.pane(pane) {
                        Some((server, own)) => parvi_term::search::capture(server.backend(), own)
                            .await
                            .ok(),
                        None => None,
                    };
                    let frame = contract::history(id, pane, capture.as_deref());
                    let _ = outbox.send(frame).await;
                });
            }
            Up::Seen { session, event } => hub.event_seen(&session, event),
        }
    }
}

impl Channel for Terminal {
    fn receive(&mut self, frame: Vec<u8>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            match contract::decode_up(&frame) {
                Ok(up) => self.handle(up).await,
                Err(error) => eprintln!("parvi: bad frame: {error}"),
            }
        })
    }

    fn next(&mut self) -> BoxFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            tokio::select! {
                frame = self.outgoing.recv() => frame,
                changed = self.changes.changed() => {
                    changed.ok()?;
                    Some(sessions(&self.hub, self.me))
                }
                changed = self.launcher_changes.changed() => {
                    changed.ok()?;
                    let config = self.launcher_changes.borrow_and_update().clone();
                    Some(frames::launcher(&config, &self.hub.users(), self.hub.notify_off()))
                }
            }
        })
    }
}

impl Drop for Terminal {
    /// The client went: its tasks stop and every seat it held goes back.
    fn drop(&mut self) {
        let tasks = std::mem::take(&mut self.subscriptions)
            .into_values()
            .chain(self.overview.take())
            .chain(self.searching.take());
        for task in tasks {
            task.abort();
        }
        let (hub, me) = (self.hub.clone(), self.me);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { hub.release_all(me).await });
        }
    }
}

/// Runs one search over every pane, sending matches as they are found and
/// then how it ended.
async fn search(hub: Arc<Hub>, id: u32, query: String, outbox: mpsc::Sender<Vec<u8>>) {
    let panes = hub
        .sessions()
        .iter()
        .flat_map(|s| s.panes.iter().map(|&(pane, _, _)| pane))
        .filter_map(|id| {
            let (server, pane) = hub.pane(id)?;
            Some(Target {
                backend: server.engine.backend().clone(),
                pane,
                id,
            })
        })
        .collect();
    let (found, mut matches) = mpsc::channel(64);
    let forward = {
        let outbox = outbox.clone();
        async move {
            while let Some(found) = matches.recv().await {
                if outbox.send(frames::found(id, &found)).await.is_err() {
                    break;
                }
            }
        }
    };
    let (summary, ()) = tokio::join!(parvi_term::search::search(panes, &query, found), forward);
    let _ = outbox.send(frames::searched(id, &summary)).await;
}

/// Sends one pane's screen and then its updates to a client, until the pane
/// goes or the client does.
async fn forward(hub: Arc<Hub>, pane: PaneId, outbox: mpsc::Sender<Vec<u8>>) {
    let Some(mut stream) = hub.subscribe(pane) else {
        let _ = outbox.send(contract::pane_gone(pane)).await;
        return;
    };
    if let Some(screen) = stream.screen.take()
        && outbox.send(frame(pane, &screen)).await.is_err()
    {
        return;
    }
    loop {
        let update = match stream.updates.recv().await {
            Ok(update) => update,
            // Fell behind: a fresh screen replaces everything missed.
            Err(broadcast::error::RecvError::Lagged(_)) => match hub.screen(pane) {
                Some(screen) => screen,
                None => continue,
            },
            Err(broadcast::error::RecvError::Closed) => {
                let _ = outbox.send(contract::pane_gone(pane)).await;
                return;
            }
        };
        if outbox.send(frame(pane, &update)).await.is_err() {
            return;
        }
    }
}

/// Sends the overview: every session's first pane as text, when it changed,
/// checked every [`OVERVIEW_EVERY`], at most [`OVERVIEW_BATCH`] at a time. Runs
/// until aborted or the client goes; a slow client delays the next check
/// rather than queueing more.
async fn thumbnails(hub: Arc<Hub>, outbox: mpsc::Sender<Vec<u8>>) {
    let mut seen = HubSent::default();
    let mut tick = tokio::time::interval(OVERVIEW_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        for thumb in hub.thumbnails(&mut seen, OVERVIEW_BATCH) {
            let frame = contract::thumbnail(thumb.pane, thumb.cols, thumb.rows, &thumb.text);
            if outbox.send(frame).await.is_err() {
                return;
            }
        }
    }
}

/// The sessions frame, with each session's last agent event.
fn sessions(hub: &Hub, me: u64) -> Vec<u8> {
    let list = hub.sessions();
    frames::sessions(&list, &hub.events(&list), me)
}

fn frame(pane: PaneId, update: &PaneUpdate) -> Vec<u8> {
    match update {
        PaneUpdate::Screen { cols, rows, bytes } => contract::screen(pane, *cols, *rows, bytes),
        PaneUpdate::Output(bytes) => contract::output(pane, bytes),
    }
}
