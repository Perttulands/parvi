//! Every node the page hears from, each a [`Remote`]: the sessions it
//! lists, its panes' screens, and what was asked of it. Pane ids are only
//! unique on one node, so everything here is addressed by (node, pane).
//! The app's link decides which nodes there are; the page hears each one
//! open, go and leave through [`Page::link`](parvi_app_kit::Page::link).
//!
//! Screens are fed on the network thread by the page's tap, so a busy pane
//! never waits on the UI; the tap wakes the page once per paint.

use crate::remote::{Match, Notice, Remote, Screens, SharedScreen, feed};
use crate::store::sessions::{self, Key};
use crate::store::workspace::{Binding, LinkState, TileState, tile_state};
use parking_lot::Mutex;
use parvi_app_kit::link::socket::Tap;
use parvi_terminals_contract::{Launch, OUTPUT, SCREEN, Session, decode_down};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Each node's screens, by node: the page's half and the tap's.
type Fed = Arc<Mutex<HashMap<String, Screens>>>;

#[derive(Default)]
pub struct Remotes {
    /// The serving node first, then the others as they came.
    list: Vec<Remote>,
    /// The serving node, as the app last named it.
    here: Option<String>,
    fed: Fed,
    /// Every session every node listed last, serving node first.
    pub all: Vec<Session>,
}

impl Remotes {
    /// The tap that feeds every node's screens: `wake` hears the first
    /// change since the page last painted.
    pub fn tap(&self, wake: async_channel::Sender<()>) -> Tap {
        let fed = self.fed.clone();
        Arc::new(move |node: &str, kind: u8, body: &[u8]| {
            if kind != SCREEN && kind != OUTPUT {
                return false;
            }
            let Some(down) = decode_down(kind, body) else {
                return true;
            };
            let screens = fed.lock().get(node).cloned();
            if let Some(screens) = screens
                && feed(&screens, &down)
            {
                let _ = wake.try_send(());
            }
            true
        })
    }

    /// The serving node, once the app has reached it.
    pub fn here_id(&self) -> Option<&str> {
        self.here.as_deref()
    }

    pub fn clients(&self) -> &[Remote] {
        &self.list
    }

    pub fn get(&self, node: &str) -> Option<&Remote> {
        self.list.iter().find(|c| c.node == node)
    }

    pub fn get_mut(&mut self, node: &str) -> Option<&mut Remote> {
        self.list.iter_mut().find(|c| c.node == node)
    }

    /// The link to `node` changed: open, gone for now, or no longer
    /// followed (`None`). `first` is the serving node, which leads.
    pub fn link(
        &mut self,
        node: &str,
        state: Option<LinkState>,
        first: Option<&str>,
    ) -> Vec<Notice> {
        let notices = match state {
            None => {
                self.fed.lock().remove(node);
                let mut gone = vec![];
                if let Some(i) = self.list.iter().position(|c| c.node == node) {
                    gone = self.list.remove(i).closed();
                }
                gone
            }
            Some(state) => {
                if self.get(node).is_none() {
                    let screens = Screens::default();
                    self.fed.lock().insert(node.to_string(), screens.clone());
                    self.list.push(Remote::new(node, screens));
                }
                let c = self.get_mut(node).expect("just made");
                c.state = state;
                match state {
                    LinkState::Open => {
                        c.opened();
                        vec![]
                    }
                    _ => c.closed(),
                }
            }
        };
        if first.is_some() {
            self.here = first.map(str::to_string);
        }
        if let Some(first) = first
            && let Some(i) = self.list.iter().position(|c| c.node == first)
        {
            let c = self.list.remove(i);
            self.list.insert(0, c);
        }
        self.collect();
        notices
    }

    /// A frame from `node`, already decoded.
    pub fn frame(&mut self, node: &str, down: parvi_terminals_contract::Down) -> Vec<Notice> {
        let Some(c) = self.get_mut(node) else {
            return vec![];
        };
        let notices = c.frame(down);
        self.collect();
        notices
    }

    /// Every frame waiting for its node, by node, in order.
    pub fn outbox(&mut self) -> Vec<(String, Vec<Vec<u8>>)> {
        self.list
            .iter_mut()
            .filter(|c| !c.out.is_empty())
            .map(|c| (c.node.clone(), std::mem::take(&mut c.out)))
            .collect()
    }

    fn collect(&mut self) {
        self.all = self
            .list
            .iter()
            .flat_map(|c| c.sessions.iter().cloned())
            .collect();
    }

    pub fn session(&self, key: &Key) -> Option<&Session> {
        sessions::find(&self.all, key)
    }

    /// What a window bound to this session shows.
    pub fn tile<'a>(&'a self, binding: Option<&'a Binding>) -> TileState<'a> {
        let node = binding.and_then(|b| self.get(&b.key.node));
        tile_state(binding, node.map(|c| (&c.sessions[..], c.state, c.known)))
    }

    pub fn screen(&self, node: &str, pane: u32) -> Option<SharedScreen> {
        self.get(node)?.screen(pane)
    }

    pub fn ended_screen(&self, key: &Key, pane: Option<u32>) -> Option<SharedScreen> {
        self.get(&key.node)?.ended_screen(key, pane)
    }

    pub fn retain_ended_screens(&mut self, bindings: &HashSet<Key>) {
        for client in &mut self.list {
            client.retain_ended_screens(bindings);
        }
    }

    pub fn input(&mut self, node: &str, pane: u32, bytes: &[u8]) {
        if let Some(c) = self.get_mut(node) {
            c.input(pane, bytes);
        }
    }

    pub fn holds(&self, node: &str, pane: u32) -> bool {
        self.get(node).is_some_and(|c| c.holds(pane))
    }

    pub fn claim(&mut self, node: &str, pane: u32, cols: u16, rows: u16) {
        if let Some(c) = self.get_mut(node) {
            c.claim(pane, cols, rows);
        }
    }

    pub fn release(&mut self, node: &str, pane: u32) {
        if let Some(c) = self.get_mut(node) {
            c.release(pane);
        }
    }

    /// Claims even if this app already holds the seat at this grid: the
    /// node is asked again, and the seat stays where it is meanwhile.
    pub fn claim_anew(&mut self, node: &str, pane: u32, cols: u16, rows: u16) {
        if let Some(c) = self.get_mut(node) {
            c.claim_anew(pane, cols, rows);
        }
    }

    pub fn give_back(&mut self, node: &str, pane: u32) {
        if let Some(c) = self.get_mut(node) {
            c.give_back(pane);
        }
    }

    pub fn given_back(&self, node: &str, pane: u32) -> bool {
        self.get(node).is_some_and(|c| c.given_back(pane))
    }

    pub fn history(&mut self, node: &str, pane: u32) -> Option<u32> {
        self.get_mut(node)?.history(pane)
    }

    pub fn paste_to(
        &mut self,
        node: &str,
        pane: u32,
        text: &str,
        enter: bool,
        ready: bool,
    ) -> Result<u32, String> {
        match self.get_mut(node) {
            Some(c) => c.paste_to(pane, text, enter, ready),
            None => Err("That device is away.".into()),
        }
    }

    pub fn delete(&mut self, node: &str, session_id: &str) -> Option<u32> {
        self.get_mut(node)?.delete(session_id)
    }

    pub fn rename(&mut self, node: &str, session_id: &str, name: &str) -> Option<u32> {
        self.get_mut(node)?.rename(session_id, name)
    }

    /// Turns tmux's mouse mode on or off on every server a node follows.
    pub fn mouse(&mut self, node: &str, on: bool) -> Option<u32> {
        self.get_mut(node)?.mouse(on)
    }

    /// Tells a node the operator looked at one of its sessions while it
    /// showed the event heard at `at`.
    pub fn seen(&mut self, s: &Session, at: u64) {
        let id = s.id.clone();
        if let Some(c) = self.get_mut(&s.node) {
            c.seen(&id, at);
        }
    }

    /// Launches on one node; `None` when it is not reached now.
    pub fn launch_on(&mut self, node: &str, l: Launch) -> Option<u32> {
        self.get_mut(node)?.launch(l)
    }

    /// Subscribes exactly these panes on each node.
    pub fn watch(&mut self, panes: &HashMap<String, HashSet<u32>>) {
        let none = HashSet::new();
        for c in &mut self.list {
            let node = c.node.clone();
            c.watch(panes.get(&node).unwrap_or(&none));
        }
    }

    /// The overview stopped: every node's thumbnails go.
    pub fn overview_off(&mut self) {
        for c in &mut self.list {
            c.overview_off();
        }
    }

    pub fn thumb(&self, node: &str, pane: u32) -> Option<&str> {
        Some(self.get(node)?.thumbs.get(&pane)?.text.as_str())
    }

    /// Searches every node that answers (an empty query stops them).
    pub fn search(&mut self, query: &str) {
        for c in &mut self.list {
            c.search(query);
        }
    }

    /// Every match so far, and, once every node has finished, the totals:
    /// matches, panes read, and whether any node stopped early.
    pub fn found(&self) -> Option<Found<'_>> {
        let searches: Vec<_> = self
            .list
            .iter()
            .filter(|c| c.state == LinkState::Open)
            .filter_map(|c| c.search.as_ref())
            .collect();
        if searches.is_empty() {
            return None;
        }
        let matches = searches.iter().flat_map(|s| &s.matches).collect();
        let done = searches.iter().try_fold((0, 0, false), |acc, s| {
            s.done.map(|(m, p, t)| (acc.0 + m, acc.1 + p, acc.2 || t))
        });
        Some((matches, done))
    }

    pub fn session_of_pane(&self, node: &str, pane: u32) -> Option<&Session> {
        self.get(node)?.session_of_pane(pane)
    }
}

/// Every match so far, and the totals once every node has finished:
/// matches, panes read, whether any node stopped early.
pub type Found<'a> = (Vec<&'a Match>, Option<(u32, u32, bool)>);

/// A pane, by the node it is on: pane ids are only unique on one node.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PaneAt {
    pub node: String,
    pub pane: u32,
}
