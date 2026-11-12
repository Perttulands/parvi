//! Reports the node's agent tools and skills, and starts checkout syncs.
mod checkout;
mod home;
mod report;
mod roles;
mod run;
mod skills;
mod sync;
mod tools;

use anyhow::{Context, Result};
use parvi_agents_contract::{FAILED, FETCH_FAILED, REPORT, SYNC, SYNCED, WATCH};
use parvi_core::Error;
use parvi_node_kit::args::{object, text};
use parvi_node_kit::{Area, BoxFuture, Built, Channel, Client, Command, Listing, Setup};
use parvi_term::Sessions;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task::JoinHandle;

pub use checkout::Fetch;
pub use home::Home;
pub use report::{Report, gather};
pub use sync::sync;

pub const NAME: &str = "agents";
pub const LISTING: Listing = Listing {
    name: NAME,
    kinds: &[0x80..=0x8f],
    commands: &[
        "agents_show",
        "agents_sync",
        "role_list",
        "role_inspect",
        "role_usage",
        "role_new",
        "role_edit",
        "role_retire",
        "role_restore",
        "role_delete",
        "role_characters",
    ],
    build,
};

struct AgentsArea {
    home: Option<Home>,
    sessions: Arc<dyn Sessions>,
    node: String,
    roles: Arc<roles::Service>,
    sync_state: Arc<SyncState>,
}

struct SyncState {
    results: Mutex<HashMap<PathBuf, Fetch>>,
    serial: Mutex<()>,
    changes: broadcast::Sender<()>,
}
impl Default for SyncState {
    fn default() -> Self {
        Self {
            results: Mutex::new(HashMap::new()),
            serial: Mutex::new(()),
            changes: broadcast::channel(32).0,
        }
    }
}
impl SyncState {
    async fn report(&self, home: &Home) -> Report {
        let mut report = gather(home, Fetch::Local).await;
        let results = self.results.lock().await.clone();
        for checkout in &mut report.checkouts {
            checkout.fetch = results.get(&checkout.path).cloned().unwrap_or(Fetch::Local);
        }
        report
    }

    async fn sync(
        &self,
        home: &Home,
        sessions: &dyn Sessions,
        shown: &str,
    ) -> Result<String, Error> {
        let path = sync::checkout(home, shown).map_err(|e| Error::new(FAILED, format!("{e:#}")))?;
        let _serial = self.serial.lock().await;
        self.results
            .lock()
            .await
            .insert(path.clone(), Fetch::Fetching);
        let _ = self.changes.send(());
        let result = run::fetch(home, &path).await;
        self.results.lock().await.insert(
            path.clone(),
            match &result {
                Ok(()) => Fetch::Fetched,
                Err(error) => Fetch::Failed(error.clone()),
            },
        );
        let _ = self.changes.send(());
        result.map_err(|error| Error::new(FETCH_FAILED, error))?;
        sync::sync_fetched(sessions, &path)
            .await
            .map_err(|e| Error::new(FAILED, format!("{e:#}")))
    }
}

/// Overrides the probed home and executable path for a node environment.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    home: Option<PathBuf>,
    path: Option<Vec<PathBuf>>,
}

pub fn build(setup: &Setup) -> Result<Built> {
    let section: Section = setup.config.section(NAME)?;
    let fixture_codex = section
        .home
        .as_ref()
        .map(|p| setup.config.path(p).join(".codex"));
    let mut home = Home::from_env();
    if let Some(dir) = section.home {
        home = Some(Home {
            dir: setup.config.path(&dir),
            path: home.map(|h| h.path).unwrap_or_default(),
        });
    }
    if let (Some(home), Some(path)) = (&mut home, section.path) {
        home.path = path.iter().map(|p| setup.config.path(p)).collect();
    }
    let root = parvi_mission_engine::Store::root_in(&setup.dir);
    let roles = Arc::new(roles::Service::new(root, fixture_codex));
    let node = setup.node.id.to_string();
    let sync_state = Arc::new(SyncState::default());
    Built::then(move |provided| {
        Ok(Arc::new(AgentsArea {
            home,
            node,
            roles,
            sync_state,
            sessions: provided
                .get::<dyn Sessions>()
                .context("agents sync needs sessions")?,
        }))
    })
}

impl Area for AgentsArea {
    fn routes(&self) -> axum::Router {
        roles::routes(self.roles.clone())
    }
    fn channel(&self, _: Client) -> Option<Box<dyn Channel>> {
        let (out, incoming) = mpsc::channel(32);
        Some(Box::new(Connection {
            home: self.home.clone(),
            sessions: self.sessions.clone(),
            out,
            incoming,
            watch: None,
            sync_state: self.sync_state.clone(),
        }))
    }
    fn commands(&self) -> Vec<Command> {
        let (home, node, sync_state) = (
            self.home.clone(),
            self.node.clone(),
            self.sync_state.clone(),
        );
        let show = Command::new(
            "agents_show",
            "Report this node's agent tools, skills and checkouts.",
            || object(json!({}), &[]),
            move |_| {
                let (home, node, sync_state) = (home.clone(), node.clone(), sync_state.clone());
                async move {
                    let home =
                        home.ok_or_else(|| Error::new(FAILED, "this node has no home folder"))?;
                    let report = sync_state.report(&home).await;
                    let mut value: Value = serde_json::from_str(&report.json(&home))
                        .map_err(|e| Error::new(FAILED, e.to_string()))?;
                    value["node"] = json!(node);
                    Ok(value)
                }
            },
        );
        let (home, sessions, node, sync_state) = (
            self.home.clone(),
            self.sessions.clone(),
            self.node.clone(),
            self.sync_state.clone(),
        );
        let sync = Command::new(
            "agents_sync",
            "Sync a skills checkout in a terminal session.",
            || object(json!({"checkout":{"type":"string"}}), &["checkout"]),
            move |args| {
                let (home, sessions, node, sync_state) = (
                    home.clone(),
                    sessions.clone(),
                    node.clone(),
                    sync_state.clone(),
                );
                async move {
                    let home =
                        home.ok_or_else(|| Error::new(FAILED, "this node has no home folder"))?;
                    let checkout = text(&args, "checkout")
                        .map_err(|e| Error::new(FAILED, e.to_string()))?
                        .to_owned();
                    // A caller can disconnect while git is running. Finish the
                    // operation so its shared result cannot remain "fetching".
                    let session = tokio::spawn(async move {
                        sync_state.sync(&home, sessions.as_ref(), &checkout).await
                    })
                    .await
                    .map_err(|error| Error::new(FAILED, error.to_string()))??;
                    Ok(json!({"node":node,"session":session}))
                }
            },
        );
        let mut commands = vec![show, sync];
        commands.extend(roles::commands(self.roles.clone(), self.node.clone()));
        commands
    }
}
#[derive(Debug, PartialEq, Eq)]
enum Request {
    Watch(bool),
    Sync { id: u32, checkout: String },
}

fn decode(frame: &[u8]) -> Option<Request> {
    match frame {
        [WATCH, on] => Some(Request::Watch(*on != 0)),
        [SYNC, rest @ ..] if rest.len() >= 4 => Some(Request::Sync {
            id: u32::from_le_bytes(rest[..4].try_into().ok()?),
            checkout: std::str::from_utf8(&rest[4..]).ok()?.to_owned(),
        }),
        _ => None,
    }
}

fn report_frame(json: &str) -> Vec<u8> {
    [&[REPORT][..], json.as_bytes()].concat()
}

fn synced_frame(id: u32, answer: Result<String, Error>) -> Vec<u8> {
    let ok = answer.is_ok();
    let text = match answer {
        Ok(session) => session,
        Err(error) => serde_json::to_string(&error).expect("error serializes"),
    };
    [
        &[SYNCED][..],
        &id.to_le_bytes(),
        &[u8::from(ok)],
        text.as_bytes(),
    ]
    .concat()
}

struct Connection {
    home: Option<Home>,
    sessions: Arc<dyn Sessions>,
    out: mpsc::Sender<Vec<u8>>,
    incoming: mpsc::Receiver<Vec<u8>>,
    watch: Option<JoinHandle<()>>,
    sync_state: Arc<SyncState>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(task) = self.watch.take() {
            task.abort();
        }
    }
}
impl Channel for Connection {
    fn receive(&mut self, frame: Vec<u8>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Some(home) = self.home.clone() else {
                return;
            };
            match decode(&frame) {
                Some(Request::Watch(on)) => {
                    if let Some(task) = self.watch.take() {
                        task.abort();
                    }
                    if on {
                        let out = self.out.clone();
                        let sync_state = self.sync_state.clone();
                        let mut changes = sync_state.changes.subscribe();
                        self.watch = Some(tokio::spawn(async move {
                            loop {
                                let report = sync_state.report(&home).await;
                                if out.send(report_frame(&report.json(&home))).await.is_err() {
                                    return;
                                }
                                match changes.recv().await {
                                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => (),
                                    Err(broadcast::error::RecvError::Closed) => return,
                                }
                            }
                        }));
                    }
                }
                Some(Request::Sync { id, checkout }) => {
                    let (out, sessions, sync_state) = (
                        self.out.clone(),
                        self.sessions.clone(),
                        self.sync_state.clone(),
                    );
                    tokio::spawn(async move {
                        let answer = sync_state.sync(&home, sessions.as_ref(), &checkout).await;
                        let _ = out.send(synced_frame(id, answer)).await;
                    });
                }
                _ => {}
            }
        })
    }
    fn next(&mut self) -> BoxFuture<'_, Option<Vec<u8>>> {
        Box::pin(self.incoming.recv())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_watch_and_sync() {
        assert_eq!(decode(&[WATCH, 1]), Some(Request::Watch(true)));
        assert_eq!(decode(&[WATCH, 0]), Some(Request::Watch(false)));
        assert_eq!(
            decode(b"\x81\x07\0\0\0~/kit"),
            Some(Request::Sync {
                id: 7,
                checkout: "~/kit".into()
            })
        );
        assert_eq!(decode(&[WATCH]), None);
        assert_eq!(decode(&[SYNC, 1]), None);
        assert_eq!(decode(&[SYNC, 0, 0, 0, 0, 0xff]), None);
        assert_eq!(decode(&[0x8a]), None);
    }

    #[test]
    fn encodes_report_and_synced() {
        assert_eq!(report_frame("{}"), b"\x88{}");
        assert_eq!(
            synced_frame(2, Ok("sync-kit".into())),
            b"\x89\x02\0\0\0\x01sync-kit"
        );
        let failed = synced_frame(3, Err(Error::new(FAILED, "refused")));
        assert_eq!(&failed[..6], b"\x89\x03\0\0\0\0");
        assert_eq!(
            serde_json::from_slice::<Error>(&failed[6..]).unwrap(),
            Error::new(FAILED, "refused")
        );
    }
}
