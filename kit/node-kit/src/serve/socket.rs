//! One client's `/ws`: the hello first, then each area's channel and view
//! control's ([`crate::view`]). A frame
//! from the client goes to the area whose kinds hold its kind byte; every
//! channel's pushes go out as they come. When the client goes, its
//! channels are dropped, which gives back whatever it held. A client that
//! stays silent through pings (a device asleep or off the network) has gone.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::future::{self, Either};
use parvi_core::frame::{self, Kinds};

use super::{Shared, hello};
use crate::{BoxFuture, Channel, Client};

/// How often a connection is pinged, and how long its client may stay
/// silent before it is taken for gone.
static LIVENESS: Mutex<(Duration, Duration)> =
    Mutex::new((Duration::from_secs(15), Duration::from_secs(45)));

/// Sets how often connections are pinged and how long a silent client is
/// kept, for a test that cannot wait the real ones out.
#[doc(hidden)]
pub fn set_liveness(ping_every: Duration, silent_for: Duration) {
    *LIVENESS.lock().unwrap() = (ping_every, silent_for);
}

enum Event {
    /// From the client; `None` once it has gone.
    Received(Option<Vec<u8>>),
    /// The client said something other than a frame (a pong).
    Heard,
    /// Time to ping, or to give up on a silent client.
    Ping,
    /// A channel's push; `None` once that channel has ended.
    Pushed(Option<Vec<u8>>),
}

pub(super) async fn connection(mut socket: WebSocket, shared: Arc<Shared>) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let client = Client {
        id: NEXT.fetch_add(1, Ordering::Relaxed),
    };
    let first = hello(&shared.node, &shared.off);
    if socket.send(Message::Binary(first.into())).await.is_err() {
        return;
    }
    let mut channels: Vec<(Kinds, Box<dyn Channel>)> = shared
        .areas
        .iter()
        .filter_map(|(listed, area)| Some((listed.kinds, area.channel(client)?)))
        .chain([(frame::VIEW, shared.views.channel(client))])
        .collect();
    let (ping_every, silent_for) = *LIVENESS.lock().unwrap();
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + ping_every, ping_every);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heard = tokio::time::Instant::now();
    loop {
        let event = {
            let pushes = match channels.is_empty() {
                true => Either::Left(future::pending()),
                false => {
                    Either::Right(future::select_all(channels.iter_mut().map(
                        |(_, channel)| -> BoxFuture<'_, Option<Vec<u8>>> { channel.next() },
                    )))
                }
            };
            tokio::select! {
                message = socket.recv() => match message {
                    Some(Ok(Message::Binary(frame))) => Event::Received(Some(frame.to_vec())),
                    Some(Ok(_)) => Event::Heard,
                    _ => Event::Received(None),
                },
                (pushed, ..) = pushes => Event::Pushed(pushed),
                _ = ping.tick() => Event::Ping,
            }
        };
        if matches!(event, Event::Received(_) | Event::Heard) {
            heard = tokio::time::Instant::now();
        }
        match event {
            Event::Received(None) | Event::Pushed(None) => break,
            Event::Heard => {}
            Event::Ping => {
                if heard.elapsed() >= silent_for {
                    break;
                }
                let sent = tokio::time::timeout(
                    silent_for,
                    socket.send(Message::Ping(Default::default())),
                );
                if !matches!(sent.await, Ok(Ok(()))) {
                    break;
                }
            }
            Event::Pushed(Some(frame)) => {
                // A client that stopped reading fills the socket; one
                // silent as long as that has gone.
                let sent =
                    tokio::time::timeout(silent_for, socket.send(Message::Binary(frame.into())));
                if !matches!(sent.await, Ok(Ok(()))) {
                    break;
                }
            }
            Event::Received(Some(frame)) => {
                let Some(&kind) = frame.first() else {
                    continue;
                };
                match channels
                    .iter_mut()
                    .find(|(kinds, _)| frame::owns(kinds, kind))
                {
                    Some((_, channel)) => channel.receive(frame).await,
                    None => eprintln!("parvi: no area takes frame kind {kind:#04x}"),
                }
            }
        }
    }
}
