//! The fleet recovers by itself (#163): a peer and the serving node are
//! killed and started again on their addresses, and every link comes back
//! without the operator asking, the away node named meanwhile by the hello
//! it gave last. The nodes are fakes on loopback that speak the hello,
//! nodes and one area's frame, the serving one piping `/ws?node=` to its
//! peer as a real node does; a killed fake drops every socket it held.

use super::Fleet;
use crate::link::client::{LinkState, Notice};
use crate::link::frames::NODES;
use crate::link::socket::{Event, Net};
use crate::settings::Place;
use futures_util::{SinkExt, StreamExt};
use parvi_core::PROTOCOL;
use parvi_core::frame::HELLO;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::tungstenite::Message;

/// A frame of an area's, which each fake sends once a socket opens.
const SESSIONS: u8 = 0x01;
const NODES_ON: u8 = 0x1b;

/// One fake node: who it is, where it listens, and its peer, if it serves.
#[derive(Clone)]
struct Who {
    node: &'static str,
    device: &'static str,
    session: &'static str,
}

struct Fake {
    who: Who,
    address: SocketAddr,
    up: Arc<AtomicBool>,
    peer: Option<(Who, SocketAddr, Arc<AtomicBool>)>,
    rt: tokio::runtime::Handle,
    task: Option<JoinHandle<()>>,
}

impl Fake {
    /// Listens on a free loopback port.
    fn start(rt: &tokio::runtime::Handle, who: Who, peer: Option<&Fake>) -> Fake {
        let listener = rt
            .block_on(TcpListener::bind("127.0.0.1:0"))
            .expect("bind a fake node");
        let mut f = Fake {
            address: listener.local_addr().unwrap(),
            who,
            up: Arc::new(AtomicBool::new(false)),
            peer: peer.map(|p| (p.who.clone(), p.address, p.up.clone())),
            rt: rt.clone(),
            task: None,
        };
        f.serve(listener);
        f
    }

    /// Starts again where it listened before.
    fn restart(&mut self) {
        let listener = self
            .rt
            .block_on(TcpListener::bind(self.address))
            .expect("bind the fake node again");
        self.serve(listener);
    }

    /// Kills it: the listener and every socket it held go at once.
    fn stop(&mut self) {
        self.up.store(false, Ordering::SeqCst);
        if let Some(t) = self.task.take() {
            t.abort();
            let _ = self.rt.block_on(t);
        }
    }

    fn serve(&mut self, listener: TcpListener) {
        self.up.store(true, Ordering::SeqCst);
        let (who, peer) = (self.who.clone(), self.peer.clone());
        self.task = Some(self.rt.spawn(async move {
            // Dropped with the task, so a kill takes every socket with it.
            let mut sockets = JoinSet::new();
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                sockets.spawn(socket(stream, who.clone(), peer.clone()));
            }
        }));
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop();
    }
}

fn frame(kind: u8, body: serde_json::Value) -> Message {
    let mut f = vec![kind];
    f.extend_from_slice(body.to_string().as_bytes());
    Message::Binary(f.into())
}

/// One client socket: the node itself, or its peer piped through it.
async fn socket(
    stream: tokio::net::TcpStream,
    who: Who,
    peer: Option<(Who, SocketAddr, Arc<AtomicBool>)>,
) {
    // The request line names the node asked for, read without taking it.
    let mut head = [0u8; 256];
    let mut n = 0;
    while !head[..n].contains(&b'\n') && n < head.len() {
        let Ok(more @ 1..) = stream.peek(&mut head).await else {
            return;
        };
        if more == n {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        n = more;
    }
    let line = String::from_utf8_lossy(&head[..n]);
    let target = line
        .split_whitespace()
        .nth(1)
        .and_then(|path| path.split_once("node="))
        .map(|(_, node)| node.to_string());
    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    match (target, &peer) {
        (Some(t), Some((p, address, _))) if t == p.node => pipe(ws, *address).await,
        _ => be(ws, who, peer).await,
    }
}

async fn be(
    mut ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    who: Who,
    peer: Option<(Who, SocketAddr, Arc<AtomicBool>)>,
) {
    let hello = serde_json::json!({
        "node": who.node, "device": who.device, "environment": "linux",
        "version": "test", "protocol": PROTOCOL,
    });
    let sessions = serde_json::json!([{
        "id": "$1", "name": who.session, "panes": [{"id": 1, "cols": 80, "rows": 24}],
    }]);
    if ws.send(frame(HELLO, hello)).await.is_err()
        || ws.send(frame(SESSIONS, sessions)).await.is_err()
    {
        return;
    }
    let mut listing = false;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            msg = ws.next() => match msg {
                Some(Ok(Message::Binary(b))) if b.first() == Some(&NODES_ON) => listing = true,
                Some(Ok(_)) => {}
                _ => return,
            },
            _ = tick.tick(), if listing => {
                // A real node pushes each change; saying it often is the same.
                let list: Vec<_> = peer.iter().map(|(p, _, up)| serde_json::json!({
                    "node": p.node, "device": p.device, "environment": "linux",
                    "version": "test", "protocol": PROTOCOL,
                    "state": if up.load(Ordering::SeqCst) { "online" } else { "offline" },
                })).collect();
                if ws.send(frame(NODES, list.into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Pipes a client to the peer, as `/ws?node=` does; a peer that does not
/// answer closes the client's socket.
async fn pipe(client: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, peer: SocketAddr) {
    let Ok((upstream, _)) = tokio_tungstenite::connect_async(format!("ws://{peer}/ws")).await
    else {
        return;
    };
    let (mut ctx, mut crx) = client.split();
    let (mut utx, mut urx) = upstream.split();
    loop {
        let sent = tokio::select! {
            m = crx.next() => match m {
                Some(Ok(m)) => utx.send(m).await,
                _ => return,
            },
            m = urx.next() => match m {
                Some(Ok(m)) => ctx.send(m).await,
                _ => return,
            },
        };
        if sent.is_err() {
            return;
        }
    }
}

/// The app's fleet, fed its events here as the shell would feed them.
struct App {
    fleet: Fleet,
    events: async_channel::Receiver<Event>,
    places: BTreeMap<String, Place>,
    /// The area frames heard, by node and kind, for a page.
    heard: Vec<(String, u8)>,
}

impl App {
    fn new(serving: SocketAddr) -> App {
        let (tx, events) = async_channel::unbounded();
        let mut fleet = Fleet::new(Net::start(), tx);
        fleet.connect(&serving.to_string());
        App {
            fleet,
            events,
            places: BTreeMap::new(),
            heard: vec![],
        }
    }

    /// Hands every event to the fleet until `done` holds; nothing is
    /// dialled by hand.
    fn until(&mut self, what: &str, done: impl Fn(&Fleet) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(&self.fleet) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.step();
        }
    }

    /// Hands every event to the fleet until a frame of `kind` from `node`
    /// has been heard.
    fn hear(&mut self, what: &str, node: &str, kind: u8) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.heard.iter().any(|(n, k)| n == node && *k == kind) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.step();
        }
    }

    /// Hands the fleet the next event, or waits a moment for one.
    fn step(&mut self) {
        match self.events.try_recv() {
            Ok(ev) => {
                for (node, n) in self.fleet.handle(ev, &self.places) {
                    if let Notice::Frame { kind, .. } = n {
                        self.heard.push((node, kind));
                    }
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn state(f: &Fleet, node: &str) -> Option<LinkState> {
    f.get(node).map(|c| c.state)
}

fn live(f: &Fleet, node: &str) -> bool {
    f.get(node).is_some_and(|c| c.state == LinkState::Open)
}

/// The device a node's client last said it is on.
fn device(f: &Fleet, node: &str) -> Option<String> {
    Some(f.get(node)?.hello.as_ref()?.device.clone())
}

const WSL: Who = Who {
    node: "wsl",
    device: "rig",
    session: "alpha",
};
const VPS: Who = Who {
    node: "vps",
    device: "vps",
    session: "bravo",
};

#[test]
fn a_peer_and_the_serving_node_come_back_without_being_asked() {
    let rt = Net::start().runtime().clone();
    let mut vps = Fake::start(&rt, VPS, None);
    let mut wsl = Fake::start(&rt, WSL, Some(&vps));
    let mut app = App::new(wsl.address);
    app.until("both nodes live", |f| live(f, "wsl") && live(f, "vps"));

    // The peer goes: its link is lost, it keeps its sessions and its name.
    vps.stop();
    app.until("the peer lost", |f| {
        state(f, "vps") == Some(LinkState::Lost)
    });
    assert!(live(&app.fleet, "wsl"), "the serving node stays");
    assert_eq!(
        device(&app.fleet, "vps").as_deref(),
        Some("vps"),
        "the away peer keeps its client and its hello"
    );
    vps.restart();
    app.until("the peer back", |f| live(f, "vps"));

    // The serving node goes, and the peer reached through it with it.
    wsl.stop();
    app.until("the serving node lost", |f| {
        f.here().state == LinkState::Lost && state(f, "vps") == Some(LinkState::Lost)
    });
    assert_eq!(device(&app.fleet, "wsl").as_deref(), Some("rig"));
    assert_eq!(app.fleet.away(), None, "with no other way in, it waits");
    wsl.restart();
    app.until("the serving node and the peer back", |f| {
        live(f, "wsl") && live(f, "vps")
    });
    assert_eq!(app.fleet.here_id(), Some("wsl"));
}

#[test]
fn while_the_serving_node_is_away_another_serves_and_then_it_goes_home() {
    let rt = Net::start().runtime().clone();
    let vps = Fake::start(&rt, VPS, None);
    let mut wsl = Fake::start(&rt, WSL, Some(&vps));
    let mut app = App::new(wsl.address);
    app.places.insert(
        "vps".into(),
        Place {
            device: "vps".into(),
            environment: "linux".into(),
            address: vps.address.to_string(),
            ..Default::default()
        },
    );
    app.until("both nodes live", |f| live(f, "wsl") && live(f, "vps"));

    wsl.stop();
    app.heard.clear();
    app.until("the peer serving", |f| {
        f.away() == Some("vps") && f.here_id() == Some("vps") && live(f, "vps")
    });
    app.hear("the peer's frames through it", "vps", SESSIONS);
    wsl.restart();
    app.until("home again", |f| {
        f.away().is_none() && live(f, "wsl") && live(f, "vps")
    });
    assert_eq!(app.fleet.here_id(), Some("wsl"));
}
