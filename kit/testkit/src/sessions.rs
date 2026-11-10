//! A stand-in for the area that provides the node's sessions, for a test of
//! an area that only lists sessions and delivers text to them: no tmux, no
//! terminals. List `(sessions::NAME, sessions::build)` with the area under
//! test, then reach the fake through [`crate::node::Node::fake_sessions`].
//!
//! It runs no programs: its seats, screens and input refuse, so an area that
//! runs seats needs the real thing.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::bail;
use parvi_node_kit::{Area, BoxFuture, Built, Setup};
use parvi_term::backend::Reply;
use parvi_term::control::PaneId;
use parvi_term::engine::{Activity, Delivering, PaneStream, SessionInfo};
use parvi_term::{Renames, Screen, SessionRef, SessionSpec, Sessions, Start};
use tokio::sync::{broadcast, watch};

/// The stand-in's area name.
pub const NAME: &str = "fake-sessions";

/// One text delivered to a session's pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivered {
    pub session: String,
    pub text: String,
    pub enter: bool,
}

#[derive(Default)]
struct State {
    sessions: Vec<SessionInfo>,
    users: Vec<String>,
    refusing: Vec<String>,
    delivered: Vec<Delivered>,
    next: u32,
}

/// The sessions a test says the node has, and what was delivered to them.
pub struct FakeSessions {
    state: Mutex<State>,
    changed: watch::Sender<u64>,
    renames: broadcast::Sender<(String, String)>,
}

impl FakeSessions {
    fn new() -> FakeSessions {
        FakeSessions {
            state: Mutex::new(State::default()),
            changed: watch::channel(0).0,
            renames: broadcast::channel(16).0,
        }
    }

    /// Adds a session on the node's own server; answers with its id.
    pub fn add(&self, name: &str) -> String {
        self.add_for(None, name)
    }

    /// Adds a session on `user`'s server (followed from now on).
    pub fn add_of(&self, user: &str, name: &str) -> String {
        self.add_for(Some(user), name)
    }

    fn add_for(&self, user: Option<&str>, name: &str) -> String {
        let mut state = self.state.lock().unwrap();
        state.next += 1;
        let id = format!("${}", state.next);
        if let Some(user) = user.filter(|u| !state.users.iter().any(|k| k == u)) {
            state.users.push(user.to_owned());
        }
        let pane = state.next;
        state.sessions.push(SessionInfo {
            id: id.clone(),
            name: name.to_owned(),
            panes: vec![(pane, 100, 30)],
            seat: None,
            facts: None,
            activity: Activity::default(),
            waiting: vec![],
            user: user.map(str::to_owned),
        });
        drop(state);
        self.changed.send_modify(|n| *n += 1);
        id
    }

    /// Renames a session and tells whoever follows renames.
    pub fn rename(&self, from: &str, to: &str) {
        for session in self.state.lock().unwrap().sessions.iter_mut() {
            if session.name == from {
                session.name = to.to_owned();
            }
        }
        let _ = self.renames.send((from.to_owned(), to.to_owned()));
        self.changed.send_modify(|n| *n += 1);
    }

    /// Makes deliveries to the session named `name` fail.
    pub fn refuse(&self, name: &str) {
        self.state.lock().unwrap().refusing.push(name.to_owned());
    }

    /// Everything delivered so far, in order.
    pub fn delivered(&self) -> Vec<Delivered> {
        self.state.lock().unwrap().delivered.clone()
    }

    fn named(&self, pane: PaneId) -> Option<SessionInfo> {
        let state = self.state.lock().unwrap();
        (state.sessions.iter())
            .find(|s| s.panes.iter().any(|&(p, _, _)| p == pane))
            .cloned()
    }
}

impl Sessions for FakeSessions {
    fn list(&self) -> Vec<SessionInfo> {
        self.state.lock().unwrap().sessions.clone()
    }

    fn users(&self) -> Vec<String> {
        self.state.lock().unwrap().users.clone()
    }

    fn watch(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn renames(&self) -> Vec<(Option<String>, Renames)> {
        vec![(None, self.renames.subscribe())]
    }

    fn active_pane<'a>(&'a self, session: &'a str) -> Reply<'a, PaneId> {
        Box::pin(async move {
            let state = self.state.lock().unwrap();
            match state.sessions.iter().find(|s| s.id == session) {
                Some(s) => Ok(s.panes[0].0),
                None => bail!("no session {session}"),
            }
        })
    }

    fn local_pane(&self, pane: PaneId) -> PaneId {
        pane
    }

    fn deliver(&self, pane: PaneId, text: String, enter: bool) -> Reply<'_, Delivering> {
        Box::pin(async move {
            let Some(session) = self.named(pane) else {
                bail!("no pane {pane}");
            };
            let mut state = self.state.lock().unwrap();
            let delivered = !state.refusing.contains(&session.name);
            if delivered {
                state.delivered.push(Delivered {
                    session: session.name,
                    text,
                    enter,
                });
            }
            Ok(Delivering::settled(delivered))
        })
    }

    fn start(&self, start: Start) -> Reply<'_, String> {
        Box::pin(async move {
            let user = (!start.user.is_empty()).then_some(start.user.as_str());
            Ok(self.add_for(user, &start.name))
        })
    }

    fn create(&self, _spec: SessionSpec) -> Reply<'_, SessionRef> {
        Box::pin(async { bail!("the fake sessions run no seats") })
    }

    fn find<'a>(&'a self, _name: &'a str) -> Reply<'a, Option<SessionRef>> {
        Box::pin(async { Ok(None) })
    }

    fn verify<'a>(&'a self, _seat: &'a SessionRef) -> Reply<'a, ()> {
        Box::pin(async { bail!("the fake sessions run no seats") })
    }

    fn kill<'a>(&'a self, _seat: &'a SessionRef) -> Reply<'a, ()> {
        Box::pin(async { bail!("the fake sessions run no seats") })
    }

    fn follow(&self, _pane: PaneId) -> Option<PaneStream> {
        None
    }

    fn screen(&self, _pane: PaneId) -> Option<Screen> {
        None
    }

    fn input(&self, pane: PaneId, _bytes: Vec<u8>) -> Reply<'_, ()> {
        Box::pin(async move { bail!("the fake sessions take no input (pane {pane})") })
    }

    fn pid(&self, pane: PaneId) -> Reply<'_, u32> {
        Box::pin(async move { bail!("the fake sessions run no programs (pane {pane})") })
    }
}

/// The fakes of the nodes running, by node id.
static FAKES: Mutex<BTreeMap<String, Arc<FakeSessions>>> = Mutex::new(BTreeMap::new());

pub(crate) fn of(node: &str) -> Option<Arc<FakeSessions>> {
    FAKES.lock().unwrap().get(node).cloned()
}

struct Fake {
    node: String,
}

impl Area for Fake {
    fn name(&self) -> &'static str {
        NAME
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        FAKES.lock().unwrap().remove(&self.node);
        Box::pin(async {})
    }
}

/// Provides a [`FakeSessions`] to the node's other areas.
pub fn build(setup: &Setup) -> anyhow::Result<Built> {
    let fake = Arc::new(FakeSessions::new());
    setup.provided.provide::<dyn Sessions>(fake.clone())?;
    let node = setup.node.id.to_string();
    FAKES.lock().unwrap().insert(node.clone(), fake);
    Built::now(Fake { node })
}
