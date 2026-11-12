//! The node's terminal sessions as other areas reach them (ARCHITECTURE.md,
//! Provided interfaces): list them and follow their changes and renames,
//! send text to one through the guarded path, start one, and run seats,
//! sessions an area owns itself, as a mission's agents are. The area that
//! follows the node's servers provides it; the rest name only this trait.
//!
//! Panes are named by their node-wide ids, as [`Sessions::list`] gives
//! them; on the node's own server those are tmux's own, so a seat's
//! [`SessionRef::pane_id`] names its pane here too.

use tokio::sync::{broadcast, watch};

use crate::backend::Reply;
use crate::control::PaneId;
use crate::engine::{Delivering, PaneStream, SessionInfo};
use crate::{Screen, SessionRef, SessionSpec};

/// One server's session renames, as (old name, new name).
pub type Renames = broadcast::Receiver<(String, String)>;

pub trait Sessions: Send + Sync {
    /// Every session this node follows, with its node-wide ids, as last
    /// listed: with nobody looking at the list that may be a while ago.
    fn list(&self) -> Vec<SessionInfo>;

    /// Every session as the servers hold them now, listed for this ask.
    fn listed(&self) -> Reply<'_, Vec<SessionInfo>>;

    /// The Unix users besides the node's own whose servers are followed.
    fn users(&self) -> Vec<String>;

    /// Changes whenever any session or pane does.
    fn watch(&self) -> watch::Receiver<u64>;

    /// Each server's renames, with whose server it is (`None`: the node's own).
    fn renames(&self) -> Vec<(Option<String>, Renames)>;

    /// The pane an unattended send goes to: a one-pane session's pane, else
    /// the active pane of its active window. Refused where neither is clear.
    fn active_pane<'a>(&'a self, session: &'a str) -> Reply<'a, PaneId>;

    /// The server's own id for a pane, as a receipt names it.
    fn local_pane(&self, pane: PaneId) -> PaneId;

    /// Queues `text` as one paste into the pane, then Enter if asked
    /// ([`crate::engine::delivery`]).
    fn deliver(&self, pane: PaneId, text: String, enter: bool) -> Reply<'_, Delivering>;

    /// Starts a session through the launcher and answers with its id once it
    /// is listed.
    fn start(&self, start: Start) -> Reply<'_, String>;

    /// Starts a seat on the node's own server: a fixed-size session its
    /// caller owns, known by an identity that outlives a node restart.
    fn create(&self, spec: SessionSpec) -> Reply<'_, SessionRef>;

    /// The seat named exactly `name` on the node's own server, if any: a
    /// caller that crashed before recording what it created finds it here.
    fn find<'a>(&'a self, name: &'a str) -> Reply<'a, Option<SessionRef>>;

    /// Refused unless the seat is still the session that was created, on
    /// the same server.
    fn verify<'a>(&'a self, seat: &'a SessionRef) -> Reply<'a, ()>;

    /// Ends a seat, only on the server that created it.
    fn kill<'a>(&'a self, seat: &'a SessionRef) -> Reply<'a, ()>;

    /// Follows a pane's screen: the screen now, then each change. `None`
    /// while the pane is not listed.
    fn follow(&self, pane: PaneId) -> Option<PaneStream>;

    /// A pane's visible cells and cursor, once its screen is read.
    fn screen(&self, pane: PaneId) -> Option<Screen>;

    /// Types bytes into a pane, keys included (`\r` is Enter).
    fn input(&self, pane: PaneId, bytes: Vec<u8>) -> Reply<'_, ()>;

    /// The process id of the program in a pane.
    fn pid(&self, pane: PaneId) -> Reply<'_, u32>;
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
