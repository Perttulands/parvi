//! What other areas need from the node's terminal sessions, as narrow as
//! their uses allow: list them and follow their changes and renames, send
//! text to one through the guarded path, start one, and run seats on the
//! node's own engine. One area provides it ([`crate::Setup::sessions`]);
//! the rest reach sessions only through it.

use std::sync::Arc;

use parvi_term::Engine;
use parvi_term::control::PaneId;
use parvi_term::engine::{Delivering, SessionInfo};
use tokio::sync::{broadcast, watch};

use crate::BoxFuture;

/// One server's session renames, as (old name, new name).
pub type Renames = broadcast::Receiver<(String, String)>;

pub trait Sessions: Send + Sync {
    /// Every session this node follows, with its node-wide ids.
    fn list(&self) -> Vec<SessionInfo>;

    /// The Unix users besides the node's own whose servers are followed.
    fn users(&self) -> Vec<String>;

    /// Changes whenever any session or pane does.
    fn watch(&self) -> watch::Receiver<u64>;

    /// Each server's renames, with whose server it is (`None`: the node's own).
    fn renames(&self) -> Vec<(Option<String>, Renames)>;

    /// The pane an unattended send goes to: a one-pane session's pane, else
    /// the active pane of its active window. Refused where neither is clear.
    fn active_pane<'a>(&'a self, session: &'a str) -> BoxFuture<'a, anyhow::Result<PaneId>>;

    /// The server's own id for a pane, as a receipt names it.
    fn local_pane(&self, pane: PaneId) -> PaneId;

    /// Queues `text` as one paste into the pane, then Enter if asked
    /// (`parvi_term::engine::delivery`).
    fn deliver(
        &self,
        pane: PaneId,
        text: String,
        enter: bool,
    ) -> BoxFuture<'_, anyhow::Result<Delivering>>;

    /// Starts a session and answers with its id once it is listed.
    fn start(&self, start: Start) -> BoxFuture<'_, anyhow::Result<String>>;

    /// The engine following the node's own server, for seats an area runs
    /// on it directly.
    fn engine(&self) -> Arc<Engine>;
}

/// A session to start: a shell in `folder`, with `command` typed into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Start {
    pub name: String,
    pub folder: String,
    /// The line typed into the shell; empty for none.
    pub command: String,
    /// Whose server; empty for the node's own.
    pub user: String,
}
