//! Every node the app follows: the serving node it dials, and each peer that
//! node lists, reached through it (`/ws?node=`). A peer that stops answering
//! keeps its client and dials again by itself. Everything a node has is
//! addressed by (node, local id). When the configured node goes away,
//! another node it last saw serves, dialled directly, until the configured
//! one answers again (`serving`).

use super::client::{Client, LinkState, Notice};
use super::frames::Peer;
use super::serving::{self, Candidate, Move, Situation};
use super::socket::{Event, Net};
use crate::settings::Place;
use std::collections::{BTreeMap, HashSet};

pub struct Fleet {
    net: Net,
    events: async_channel::Sender<Event>,
    /// The configured node's address.
    home: String,
    /// Where the serving node is dialled: the configured node's address, or
    /// another node's while that is away.
    base: String,
    /// The serving node first, then one per peer.
    clients: Vec<Client>,
    /// While another node serves: the configured node's own link,
    /// redialling with backoff until it answers.
    homeward: Option<Client>,
    /// The node serving while the configured one is away.
    away: Option<String>,
    /// The serving node's peer list, as it last said it.
    pub peers: Vec<Peer>,
    /// The serving node has listed its peers on this connection.
    pub listed: bool,
    /// Channels every node has on.
    channels: HashSet<u8>,
}

/// A peer worth dialling: it speaks this protocol, whether it answers now
/// or not (it comes back by itself).
fn followed(p: &Peer) -> bool {
    p.state != "needs-upgrade"
}

impl Fleet {
    pub fn new(net: Net, events: async_channel::Sender<Event>) -> Self {
        Self {
            clients: vec![Client::new(net.clone(), None)],
            net,
            events,
            home: String::new(),
            base: String::new(),
            homeward: None,
            away: None,
            peers: vec![],
            listed: false,
            channels: HashSet::new(),
        }
    }

    pub fn here(&self) -> &Client {
        &self.clients[0]
    }

    pub fn here_mut(&mut self) -> &mut Client {
        &mut self.clients[0]
    }

    /// The serving node's id, once it has said hello.
    pub fn here_id(&self) -> Option<&str> {
        self.here().hello.as_ref().map(|h| h.node.as_str())
    }

    pub fn clients(&self) -> &[Client] {
        &self.clients
    }

    pub fn get(&self, node: &str) -> Option<&Client> {
        if node.is_empty() {
            return None;
        }
        self.clients.iter().find(|c| c.node() == node)
    }

    fn get_mut(&mut self, node: &str) -> Option<&mut Client> {
        if node.is_empty() {
            return None;
        }
        self.clients.iter_mut().find(|c| c.node() == node)
    }

    /// The address HTTP requests go to: the serving node's.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The node serving while the configured one is away.
    pub fn away(&self) -> Option<&str> {
        self.away.as_deref()
    }

    /// The configured node's device, once it has said hello.
    pub fn home_device(&self) -> Option<&str> {
        let home = self.homeward.as_ref().unwrap_or(&self.clients[0]);
        home.hello.as_ref().map(|h| h.device.as_str())
    }

    /// Dials the configured node, forgetting the peers of the last one.
    pub fn connect(&mut self, base: &str) {
        self.home = base.to_string();
        self.base = base.to_string();
        self.homeward = None;
        self.away = None;
        self.clients.truncate(1);
        self.peers.clear();
        self.listed = false;
        self.clients[0].connect(base, self.events.clone());
    }

    /// Every link: the serving node's, each peer's, and, while away, the
    /// configured node's own.
    fn links_mut(&mut self) -> impl Iterator<Item = &mut Client> {
        self.clients.iter_mut().chain(self.homeward.iter_mut())
    }

    /// Dial every link now instead of waiting out the backoff.
    pub fn redial(&mut self) {
        for c in self.links_mut() {
            c.redial();
        }
    }

    /// Dials one node's link now.
    pub fn redial_one(&mut self, node: &str) {
        if let Some(c) = self.get_mut(node) {
            c.redial();
        }
    }

    /// Hands an event to the client whose link it came from, then decides
    /// which node serves, among the nodes last seen in `places`. Notices
    /// come back with the node they are about.
    pub fn handle(&mut self, ev: Event, places: &BTreeMap<String, Place>) -> Vec<(String, Notice)> {
        let id = match &ev {
            Event::Opened(i, _)
            | Event::Refused(i, _)
            | Event::Frame(i, ..)
            | Event::Closed(i, _) => *i,
        };
        let frame = matches!(ev, Event::Frame(..));
        let serving = self.clients[0].link_id() == id;
        let Some(c) = self.links_mut().find(|c| c.link_id() == id) else {
            return vec![];
        };
        let notices = c.handle(ev);
        let node = c.node();
        for n in &notices {
            if let Notice::Nodes(list) = n
                && serving
            {
                self.follow(list.clone());
            }
        }
        let mut out: Vec<_> = notices.into_iter().map(|n| (node.clone(), n)).collect();
        if !frame {
            out.extend(self.steer(places).map(|n| (String::new(), n)));
        }
        out
    }

    /// Fails over to another node, or goes home, when `store::serving`
    /// says so.
    fn steer(&mut self, places: &BTreeMap<String, Place>) -> Option<Notice> {
        let decided = {
            let home = self.homeward.as_ref().unwrap_or(&self.clients[0]);
            let situation = Situation {
                home: home
                    .hello
                    .as_ref()
                    .map(|h| (h.node.as_str(), h.device.as_str())),
                home_open: home.state == LinkState::Open,
                home_misses: home.misses,
                away: self.away.as_deref(),
                away_misses: if self.away.is_some() {
                    self.clients[0].misses
                } else {
                    0
                },
            };
            serving::decide(&situation, &self.candidates(places))
        };
        match decided {
            Move::Stay => None,
            Move::Through { node, address } => {
                let serving = self.serve_through(&address);
                if self.homeward.is_none() {
                    self.homeward = Some(serving);
                }
                self.away = Some(node.clone());
                Some(Notice::Serving(Some(node)))
            }
            Move::Home => {
                let home = self.homeward.take().expect("away means a homeward link");
                self.clients = vec![home];
                self.base = self.home.clone();
                self.away = None;
                self.peers.clear();
                self.listed = false;
                Some(Notice::Serving(None))
            }
        }
    }

    /// Makes a new serving link to `address`, dropping the peers reached
    /// through the old one, and hands back the old serving link.
    fn serve_through(&mut self, address: &str) -> Client {
        let mut c = Client::new(self.net.clone(), None);
        for &kind in &self.channels {
            c.channel(kind, true);
        }
        c.connect(address, self.events.clone());
        self.base = address.to_string();
        self.clients.truncate(1);
        self.peers.clear();
        self.listed = false;
        std::mem::replace(&mut self.clients[0], c)
    }

    /// The nodes last seen that a client can dial directly.
    fn candidates(&self, places: &BTreeMap<String, Place>) -> Vec<Candidate> {
        places
            .iter()
            .filter(|(_, p)| !p.address.is_empty())
            .map(|(node, p)| Candidate {
                node: node.clone(),
                device: p.device.clone(),
                address: p.address.clone(),
                online: self
                    .peers
                    .iter()
                    .any(|peer| peer.node == *node && peer.state == "online"),
            })
            .collect()
    }

    /// Keeps one client per listed peer: new ones dial, ones no longer
    /// listed go, and one that answers again is dialled at once.
    fn follow(&mut self, list: Vec<Peer>) {
        let here = self.here_id().map(str::to_string);
        let wanted: HashSet<&str> = list
            .iter()
            .filter(|p| followed(p) && Some(p.node.as_str()) != here.as_deref())
            .map(|p| p.node.as_str())
            .collect();
        let mut i = 1;
        while i < self.clients.len() {
            if wanted.contains(self.clients[i].node().as_str()) {
                i += 1;
            } else {
                self.clients.remove(i);
            }
        }
        for p in list.iter().filter(|p| wanted.contains(p.node.as_str())) {
            match self.get_mut(&p.node) {
                Some(c) => {
                    if p.state == "online" && c.state != LinkState::Open {
                        c.redial();
                    }
                }
                None => {
                    let mut c = Client::new(self.net.clone(), Some(p.node.clone()));
                    c.connect(&self.base, self.events.clone());
                    for &kind in &self.channels {
                        c.channel(kind, true);
                    }
                    self.clients.push(c);
                }
            }
        }
        self.peers = list;
        self.listed = true;
    }

    /// Turns a channel on or off on every node, and on each node that
    /// comes into reach while it is on.
    pub fn channel(&mut self, kind: u8, on: bool) {
        if on {
            self.channels.insert(kind);
        } else {
            self.channels.remove(&kind);
        }
        for c in self.links_mut() {
            c.channel(kind, on);
        }
    }

    /// Sends a frame to one node. False when it is not reached now.
    pub fn send(&self, node: &str, frame: Vec<u8>) -> bool {
        self.get(node).is_some_and(|c| c.send(frame))
    }
}

#[cfg(test)]
mod recovery;
