//! Several tmux servers as one (issue #39): the service user's own, and one
//! per Unix user the host gives Parvi access to. Each is followed by its own
//! [`Engine`]; the hub gives clients one list and one set of ids.
//!
//! The first server keeps tmux's ids, so a host with one server sees no
//! change and saved workspaces keep working. Another server's panes carry its
//! index in the top 8 bits of the pane id, and its sessions carry the user:
//! `$3@alice`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::agent_events::hooks::Hooks;
use crate::agent_events::{Event, Events, Kind};
use crate::launch::{self, Config, Launch};
use parvi_term::backend::Backend;
use parvi_term::control::PaneId;
use parvi_term::engine::{
    Delivering, Engine, PaneStream, PaneUpdate, Sent, SessionInfo, Thumbnail,
};

/// Bits of a pane id that are tmux's own; the rest say which server.
const PANE_BITS: u32 = 24;
const PANE_MASK: u32 = (1 << PANE_BITS) - 1;

/// How long a launched or renamed session may take to show in the list.
const LISTED_WITHIN: Duration = Duration::from_secs(5);

/// The most servers one hub follows.
pub const MAX_SERVERS: usize = 1 << (32 - PANE_BITS);

pub use parvi_node_kit::Renames;

/// One tmux server and who owns it.
pub struct Server {
    /// `None` for the service user's own server.
    pub user: Option<String>,
    pub engine: Arc<Engine>,
}

impl Server {
    /// The backend its engine follows, for one-shot commands.
    pub fn backend(&self) -> &dyn Backend {
        self.engine.backend().as_ref()
    }
}

/// A tmux window and the pane it currently shows.
#[derive(Debug, serde::Serialize)]
pub struct WindowInfo {
    pub index: u32,
    pub name: String,
    pub pane: u32,
}

pub struct Hub {
    servers: Vec<Server>,
    generation: watch::Sender<u64>,
    forwarders: Vec<JoinHandle<()>>,
    events: Events,
    /// How launches install completion hooks, or why they can't.
    hooks: Result<Hooks, String>,
}

/// How a mouse change went: on how many servers, and why not on the rest.
#[derive(Debug)]
pub struct Mouse {
    pub total: usize,
    pub failures: Vec<String>,
}

impl Mouse {
    pub fn applied(&self) -> usize {
        self.total - self.failures.len()
    }

    /// `mouse on, on 2 of 2 tmux servers`, then each failure.
    pub fn text(&self, on: bool) -> String {
        let mut text = format!(
            "mouse {}, on {} of {} tmux servers",
            if on { "on" } else { "off" },
            self.applied(),
            self.total
        );
        for failure in &self.failures {
            text.push_str("; ");
            text.push_str(failure);
        }
        text
    }
}

impl Hub {
    /// Follows these servers, the service user's own first. Needs a Tokio
    /// runtime: it forwards each engine's changes.
    pub fn new(servers: Vec<Server>) -> Result<Hub> {
        if servers.is_empty() || servers.len() > MAX_SERVERS {
            bail!("between 1 and {MAX_SERVERS} tmux servers");
        }
        let (generation, _) = watch::channel(0);
        let forwarders = servers
            .iter()
            .map(|server| {
                let mut changes = server.engine.watch();
                let generation = generation.clone();
                tokio::spawn(async move {
                    while changes.changed().await.is_ok() {
                        generation.send_modify(|n| *n += 1);
                    }
                })
            })
            .collect();
        Ok(Hub {
            servers,
            generation,
            forwarders,
            events: Events::default(),
            hooks: Err("this node was started without completion hooks".into()),
        })
    }

    /// Launches asked to notify install these hooks, or are told why not.
    pub fn with_hooks(mut self, hooks: Result<Hooks, String>) -> Hub {
        self.hooks = hooks;
        self
    }

    /// The engine following the service user's own server.
    pub fn own_engine(&self) -> Arc<Engine> {
        self.servers[0].engine.clone()
    }

    /// The users whose servers are followed, besides the service user.
    pub fn users(&self) -> Vec<String> {
        self.servers.iter().filter_map(|s| s.user.clone()).collect()
    }

    /// The server for a user; `""` is the service user's own.
    pub fn server(&self, user: &str) -> Result<&Server> {
        self.servers
            .iter()
            .find(|s| s.user.as_deref().unwrap_or("") == user)
            .with_context(|| format!("no tmux server for user {user:?}"))
    }

    /// The server and tmux's own id for a hub pane id.
    pub fn pane(&self, pane: PaneId) -> Option<(&Server, PaneId)> {
        let server = self.servers.get((pane >> PANE_BITS) as usize)?;
        Some((server, pane & PANE_MASK))
    }

    /// The server and tmux's own id for a hub session id.
    pub fn session<'a>(&self, session: &'a str) -> Result<(&Server, &'a str)> {
        match session.split_once('@') {
            Some((id, user)) if !user.is_empty() => Ok((self.server(user)?, id)),
            Some(_) => bail!("{session:?} is not a session id"),
            None => Ok((&self.servers[0], session)),
        }
    }

    /// Windows and their active panes for one session, read when its picker opens.
    pub async fn windows(&self, session: &str) -> Result<Vec<WindowInfo>> {
        let (server, id) = self.session(session)?;
        if !self.sessions().iter().any(|s| s.id == session) {
            bail!("session no longer exists");
        }
        let output = server
            .backend()
            .command(&[
                "list-windows",
                "-t",
                id,
                "-F",
                "#{window_index}\t#{pane_id}\t#{window_name}",
            ])
            .await?;
        output
            .lines()
            .map(|line| {
                let mut fields = line.splitn(3, '\t');
                let index = fields.next().context("missing window index")?.parse()?;
                let pane: u32 = fields
                    .next()
                    .context("missing pane")?
                    .strip_prefix('%')
                    .context("invalid pane")?
                    .parse()?;
                let name = fields.next().context("missing window name")?.to_owned();
                let pane = self.pane_id(server, pane).context("pane id too large")?;
                Ok(WindowInfo { index, name, pane })
            })
            .collect()
    }

    /// A hub session id for a server's own.
    pub fn session_id(&self, server: &Server, session: &str) -> String {
        match &server.user {
            Some(user) => format!("{session}@{user}"),
            None => session.to_owned(),
        }
    }

    fn index(&self, server: &Server) -> u32 {
        self.servers
            .iter()
            .position(|s| std::ptr::eq(s, server))
            .expect("a server of this hub") as u32
    }

    /// A hub pane id for a server's own. `None` past the ids it can carry,
    /// 16 million panes into a server's life.
    pub fn pane_id(&self, server: &Server, pane: PaneId) -> Option<PaneId> {
        (pane <= PANE_MASK).then(|| (self.index(server) << PANE_BITS) | pane)
    }

    /// Every server's sessions, with hub ids.
    pub fn sessions(&self) -> Vec<SessionInfo> {
        let mut all = Vec::new();
        for server in &self.servers {
            for mut session in server.engine.sessions() {
                session.id = self.session_id(server, &session.id);
                session.user = server.user.clone();
                session.panes = session
                    .panes
                    .into_iter()
                    .filter_map(|(id, cols, rows)| Some((self.pane_id(server, id)?, cols, rows)))
                    .collect();
                session.waiting = session
                    .waiting
                    .into_iter()
                    .filter_map(|(id, question)| Some((self.pane_id(server, id)?, question)))
                    .collect();
                all.push(session);
            }
        }
        all
    }

    /// Keeps a hook's report as the session's last event and tells every
    /// client. Refused for a session the hub doesn't list.
    pub fn record_event(&self, session: &str, kind: Kind, summary: String) -> Result<()> {
        if !self.sessions().iter().any(|s| s.id == session) {
            bail!("no session {session:?}");
        }
        self.events.record(session, kind, summary);
        self.generation.send_modify(|n| *n += 1);
        Ok(())
    }

    /// Someone looked at the session while its event was the one heard at
    /// `at`: that event is no longer news, for anyone. A newer one stays news.
    pub fn event_seen(&self, session: &str, at: u64) {
        if self.events.seen(session, at) {
            self.generation.send_modify(|n| *n += 1);
        }
    }

    /// The last event of each listed session that has one.
    pub fn events(&self, sessions: &[SessionInfo]) -> HashMap<String, Event> {
        self.events.of(sessions)
    }

    /// Why launches asked to notify get no completion hooks here, if they
    /// don't: every launcher says so.
    pub fn notify_off(&self) -> Option<&str> {
        self.hooks.as_ref().err().map(String::as_str)
    }

    /// Launches the session on the chosen user's server and returns its hub
    /// id once the list has it with a pane, so a caller can use it straight
    /// away, with what did not go as asked.
    pub async fn launch(
        &self,
        launcher: &Config,
        launch: &Launch,
    ) -> Result<(String, Option<String>)> {
        let mut changes = self.watch();
        let server = self.server(&launch.user)?;
        let hooks = self.hooks.as_ref().map_err(String::as_str);
        if let Ok(hooks) = hooks
            && launch.notify
        {
            let live: Vec<String> = self.sessions().into_iter().map(|s| s.id).collect();
            hooks.prune(&live);
        }
        let launch::Launched { id, warning } =
            launch::launch(&server.engine, launcher, launch, hooks).await?;
        let session = self.session_id(server, &id);
        let listed = async {
            while !self
                .sessions()
                .iter()
                .any(|s| s.id == session && !s.panes.is_empty())
            {
                if changes.changed().await.is_err() {
                    break;
                }
            }
        };
        tokio::time::timeout(LISTED_WITHIN, listed)
            .await
            .context("the new session did not show up")?;
        Ok((session, warning))
    }

    /// Renames a session by its hub id, on its own server, and answers once
    /// the list has the new name, so a caller can use it straight away.
    pub async fn rename(&self, session: &str, name: &str) -> Result<()> {
        let mut changes = self.watch();
        let (server, id) = self.session(session)?;
        launch::rename(server.backend(), id, name).await?;
        let listed = async {
            while !self
                .sessions()
                .iter()
                .any(|s| s.id == session && s.name == name)
            {
                if changes.changed().await.is_err() {
                    break;
                }
            }
        };
        tokio::time::timeout(LISTED_WITHIN, listed)
            .await
            .context("the new name did not show up")?;
        Ok(())
    }

    /// Turns tmux's mouse mode on or off on every server this hub follows
    /// (CHROTE's toggle, across its terminal users' sockets), then reads the
    /// sessions' facts again so their mouse badges show the change.
    pub async fn mouse(&self, on: bool) -> Mouse {
        let mut failures = Vec::new();
        for server in &self.servers {
            if let Err(error) = launch::mouse(server.backend(), on).await {
                let whose = match &server.user {
                    Some(user) => format!("{user}'s server"),
                    None => "the service user's server".to_string(),
                };
                failures.push(format!("{whose}: {error:#}"));
            }
        }
        self.refresh_facts();
        Mouse {
            total: self.servers.len(),
            failures,
        }
    }

    /// A receiver that changes whenever any server's sessions or panes do.
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }

    /// Each server's session renames, as (old name, new name), with whose
    /// server it is (`None` for the service user's own).
    pub fn renames(&self) -> Vec<(Option<String>, Renames)> {
        (self.servers.iter())
            .map(|server| (server.user.clone(), server.engine.renames()))
            .collect()
    }

    pub fn refresh_facts(&self) {
        for server in &self.servers {
            server.engine.refresh_facts();
        }
    }

    pub fn subscribe(&self, pane: PaneId) -> Option<PaneStream> {
        let (server, pane) = self.pane(pane)?;
        server.engine.subscribe(pane)
    }

    pub fn screen(&self, pane: PaneId) -> Option<PaneUpdate> {
        let (server, pane) = self.pane(pane)?;
        server.engine.screen(pane)
    }

    pub fn text(&self, pane: PaneId) -> Option<String> {
        let (server, pane) = self.pane(pane)?;
        server.engine.text(pane)
    }

    pub async fn send_input(&self, pane: PaneId, data: Vec<u8>) -> Result<()> {
        let (server, pane) = self.pane(pane).context("unknown pane")?;
        server.engine.send_input(pane, data).await
    }

    pub async fn deliver(&self, pane: PaneId, text: String, enter: bool) -> Result<Delivering> {
        let (server, pane) = self.pane(pane).context("unknown pane")?;
        server.engine.deliver(pane, text, enter).await
    }

    /// As [`Hub::deliver`], waiting first until the pane is ready for a
    /// paste (#242).
    pub async fn deliver_when_ready(
        &self,
        pane: PaneId,
        text: String,
        enter: bool,
    ) -> Result<Delivering> {
        let (server, pane) = self.pane(pane).context("unknown pane")?;
        server
            .engine
            .deliver_when_ready(pane, text, enter, parvi_term::engine::READY_WAIT)
            .await
    }

    pub async fn claim(&self, pane: PaneId, holder: u64, cols: u16, rows: u16) -> Result<()> {
        let (server, pane) = self.pane(pane).context("unknown pane")?;
        server.engine.claim(pane, holder, cols, rows).await
    }

    pub async fn release(&self, pane: PaneId, holder: u64) -> Result<()> {
        let (server, pane) = self.pane(pane).context("unknown pane")?;
        server.engine.release(pane, holder).await
    }

    pub async fn release_all(&self, holder: u64) {
        for server in &self.servers {
            server.engine.release_all(holder).await;
        }
    }

    /// Changed first panes across every server, at most `limit`. Each look
    /// starts at the next server, so a busy one can't starve the rest.
    pub fn thumbnails(&self, seen: &mut HubSent, limit: usize) -> Vec<Thumbnail> {
        seen.sent.resize_with(self.servers.len(), Sent::default);
        seen.start = (seen.start + 1) % self.servers.len();
        let mut all = Vec::new();
        for i in 0..self.servers.len() {
            let at = (seen.start + i) % self.servers.len();
            let server = &self.servers[at];
            let left = limit - all.len();
            if left == 0 {
                break;
            }
            for mut thumb in server.engine.thumbnails(&mut seen.sent[at], left) {
                if let Some(id) = self.pane_id(server, thumb.pane) {
                    thumb.pane = id;
                    all.push(thumb);
                }
            }
        }
        all
    }

    /// Stops following every server.
    pub async fn stop(&self) {
        for server in &self.servers {
            server.engine.stop().await;
        }
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        for forwarder in &self.forwarders {
            forwarder.abort();
        }
    }
}

/// What one overview has been sent, per server.
#[derive(Debug, Default)]
pub struct HubSent {
    sent: Vec<Sent>,
    start: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_term::backend::tmux::Socket;

    #[tokio::test]
    async fn the_first_server_keeps_tmux_ids_and_the_rest_carry_their_own() {
        // Sockets in a directory of its own, where no server runs.
        let dir = std::env::temp_dir().join(format!("parvi-hub-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let server = |user: Option<&str>, name: &str| Server {
            user: user.map(str::to_owned),
            engine: Arc::new(Engine::start(Socket::Path(dir.join(name)))),
        };
        let hub = Hub::new(vec![server(None, "a"), server(Some("alice"), "b")]).unwrap();
        let (own, alice) = (&hub.servers[0], &hub.servers[1]);
        assert_eq!(hub.session_id(own, "$3"), "$3");
        assert_eq!(hub.session_id(alice, "$3"), "$3@alice");
        assert_eq!(hub.pane_id(own, 7), Some(7));
        assert_eq!(hub.pane_id(alice, 7), Some((1 << 24) | 7));
        assert_eq!(hub.pane_id(alice, 1 << 24), None);

        let (found, id) = hub.session("$3@alice").unwrap();
        assert!(std::ptr::eq(found, alice) && id == "$3");
        let (found, id) = hub.session("$3").unwrap();
        assert!(std::ptr::eq(found, own) && id == "$3");
        assert!(hub.session("$3@bob").is_err());
        assert!(hub.session("$3@").is_err());
        let (found, pane) = hub.pane((1 << 24) | 7).unwrap();
        assert!(std::ptr::eq(found, alice) && pane == 7);
        assert!(hub.pane(5 << 24).is_none());
        assert_eq!(hub.users(), ["alice"]);
        hub.stop().await;
        std::fs::remove_dir(dir).unwrap();
    }
}
