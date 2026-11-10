//! One client's `/ws`: the hello first, then each area's channel. A frame
//! from the client goes to the area whose kinds hold its kind byte; every
//! channel's pushes go out as they come. When the client goes, its
//! channels are dropped, which gives back whatever it held.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::ws::{Message, WebSocket};
use futures_util::future::{self, Either};
use parvi_core::frame::{self, Kinds};

use super::{Shared, hello};
use crate::{BoxFuture, Channel, Client};

enum Event {
    /// From the client; `None` once it has gone.
    Received(Option<Vec<u8>>),
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
        .collect();
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
                    Some(Ok(_)) => continue,
                    _ => Event::Received(None),
                },
                (pushed, ..) = pushes => Event::Pushed(pushed),
            }
        };
        match event {
            Event::Received(None) | Event::Pushed(None) => break,
            Event::Pushed(Some(frame)) => {
                if socket.send(Message::Binary(frame.into())).await.is_err() {
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
