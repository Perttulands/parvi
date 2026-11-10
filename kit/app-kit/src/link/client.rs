//! The app's side of one node: the link, what the node said in its hello,
//! and the channels turned on. It knows the link's own frames and nothing
//! of pages; every other frame comes back as a notice for the page whose
//! kinds hold it.

use super::frames::{self, Peer};
use super::socket::{Event, Link, LinkId, Net, ws_url};
use parvi_core::Hello;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};

/// Every link the app opens gets its own number, so events find their client.
static LINKS: AtomicU32 = AtomicU32::new(0);

/// The link's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Connecting,
    Open,
    Lost,
    Refused,
}

/// What happened that the shell or a page may need to hear about.
#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    /// The link opened, after being lost or refused (or the first time).
    Opened,
    /// The serving node's peers, as it lists them now.
    Nodes(Vec<Peer>),
    /// Another node serves while the configured one is away; `None` once
    /// the configured node serves again.
    Serving(Option<String>),
    /// A frame of a kind the link does not own, for the page that takes it.
    Frame { kind: u8, body: Vec<u8> },
}

pub struct Client {
    net: Net,
    link: Option<Link>,
    link_id: LinkId,
    /// The node this client reaches through the serving one; `None` is the
    /// serving node itself.
    pub target: Option<String>,
    pub base: String,
    pub state: LinkState,
    pub hello: Option<Hello>,
    /// Why the node was refused, or why the socket went.
    pub reason: Option<String>,
    /// Dials missed in a row since the link was last open.
    pub misses: u32,
    /// Channels turned on, by their kind: sent again on each new socket.
    channels: HashSet<u8>,
}

impl Client {
    pub fn new(net: Net, target: Option<String>) -> Self {
        Self {
            net,
            link: None,
            link_id: 0,
            target,
            base: String::new(),
            state: LinkState::Connecting,
            hello: None,
            reason: None,
            misses: 0,
            channels: HashSet::new(),
        }
    }

    /// Dials `base`, dropping any earlier link. Events arrive on `events`.
    pub fn connect(&mut self, base: &str, events: async_channel::Sender<Event>) {
        self.link = None;
        self.link_id = LINKS.fetch_add(1, Ordering::Relaxed) + 1;
        self.base = base.to_string();
        self.state = LinkState::Connecting;
        self.reason = None;
        self.misses = 0;
        self.link = Some(
            self.net
                .open(self.link_id, ws_url(base, self.target.as_deref()), events),
        );
    }

    /// The node's id: the one it was addressed by, or the one it said.
    pub fn node(&self) -> String {
        match (&self.target, &self.hello) {
            (Some(t), _) => t.clone(),
            (None, Some(h)) => h.node.clone(),
            (None, None) => String::new(),
        }
    }

    pub fn link_id(&self) -> LinkId {
        self.link_id
    }

    pub fn runtime(&self) -> &tokio::runtime::Handle {
        self.net.runtime()
    }

    /// Dial now instead of waiting out the backoff.
    pub fn redial(&mut self) {
        if let Some(l) = &self.link {
            if self.state != LinkState::Open {
                self.state = LinkState::Connecting;
            }
            l.redial();
        }
    }

    /// Sends a frame, if the link is open. False when it is not.
    pub fn send(&self, frame: Vec<u8>) -> bool {
        let open = self.state == LinkState::Open;
        if open && let Some(l) = &self.link {
            l.send(frame);
        }
        open
    }

    pub fn handle(&mut self, ev: Event) -> Vec<Notice> {
        let id = match &ev {
            Event::Opened(i, _)
            | Event::Refused(i, _)
            | Event::Frame(i, ..)
            | Event::Closed(i, _) => *i,
        };
        if id != self.link_id {
            return vec![];
        }
        match ev {
            Event::Opened(_, hello) => {
                self.hello = Some(hello);
                self.state = LinkState::Open;
                self.misses = 0;
                self.reason = None;
                if self.target.is_none() {
                    self.send(frames::nodes_on());
                }
                for &kind in &self.channels {
                    self.send(frames::channel(kind, true));
                }
                vec![Notice::Opened]
            }
            Event::Refused(_, why) => {
                self.state = LinkState::Refused;
                self.reason = Some(why);
                vec![]
            }
            Event::Closed(_, why) => {
                self.state = LinkState::Lost;
                self.misses += 1;
                self.reason = Some(why);
                vec![]
            }
            Event::Frame(_, frames::NODES, body) => match frames::nodes(&body) {
                Some(list) if self.target.is_none() => vec![Notice::Nodes(list)],
                _ => vec![],
            },
            Event::Frame(_, kind, body) => vec![Notice::Frame { kind, body }],
        }
    }

    /// Turns a channel on or off; an open channel is turned on again on
    /// every new socket. Turning it on again asks for its state.
    pub fn channel(&mut self, kind: u8, on: bool) {
        let changed = if on {
            self.channels.insert(kind)
        } else {
            self.channels.remove(&kind)
        };
        if changed {
            self.send(frames::channel(kind, on));
        }
    }
}
