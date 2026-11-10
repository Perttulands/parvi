//! One socket to one node, on the network thread: connect, read frames, and
//! dial again with backoff when it drops.
//! It knows nothing about stores or views: it hands frames to its caller.

use super::frames::{self, Hello, PROTOCOL};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::tungstenite::Message;

/// Which link an event came from: the app numbers its links.
pub type LinkId = u32;

#[derive(Debug)]
pub enum Event {
    /// The node said hello in this client's protocol; frames follow.
    Opened(LinkId, Hello),
    /// The node speaks another protocol, or did not say hello. Nothing else
    /// will be sent on this link until it is dialled again.
    Refused(LinkId, String),
    /// A frame of a kind the link does not own: its kind, then its body.
    Frame(LinkId, u8, Vec<u8>),
    /// The socket went, or could not be opened; the link dials again by itself.
    Closed(LinkId, String),
}

/// The UI's handle on a link. Dropping it closes the socket.
pub struct Link {
    out: mpsc::UnboundedSender<Vec<u8>>,
    redial: Arc<Notify>,
}

impl Link {
    pub fn send(&self, frame: Vec<u8>) {
        let _ = self.out.send(frame);
    }

    /// Dial now instead of waiting out the backoff.
    pub fn redial(&self) {
        self.redial.notify_one();
    }
}

/// The network thread: one runtime for every link.
#[derive(Clone)]
pub struct Net {
    rt: tokio::runtime::Handle,
}

impl Net {
    pub fn start() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("parvi-net".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("network runtime");
                tx.send(rt.handle().clone()).expect("hand over the runtime");
                rt.block_on(std::future::pending::<()>());
            })
            .expect("network thread");
        Self {
            rt: rx.recv().expect("network runtime"),
        }
    }

    pub fn open(&self, id: LinkId, url: String, events: async_channel::Sender<Event>) -> Link {
        let (out, rx) = mpsc::unbounded_channel();
        let redial = Arc::new(Notify::new());
        self.rt.spawn(run(id, url, rx, redial.clone(), events));
        Link { out, redial }
    }

    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.rt
    }
}

/// The wait before dial `attempt` (0 = first retry): 1 s doubling to 30 s.
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(5)).min(30))
}

async fn run(
    id: LinkId,
    url: String,
    mut out: mpsc::UnboundedReceiver<Vec<u8>>,
    redial: Arc<Notify>,
    events: async_channel::Sender<Event>,
) {
    let mut attempt = 0u32;
    loop {
        let why = match session(id, &url, &mut out, &events).await {
            Ending::Dropped => return,
            Ending::Refused => {
                // Wait for an explicit redial: the version will not change by itself.
                tokio::select! {
                    _ = redial.notified() => {}
                    _ = drain_until_closed(&mut out) => return,
                }
                attempt = 0;
                continue;
            }
            Ending::Lost(why, was_open) => {
                if was_open {
                    attempt = 0;
                }
                why
            }
        };
        if events.send(Event::Closed(id, why)).await.is_err() {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(backoff(attempt)) => {}
            _ = redial.notified() => {}
            _ = drain_until_closed(&mut out) => return,
        }
        attempt += 1;
    }
}

/// Frames sent while there is no socket are dropped: a new socket starts
/// with fresh subscriptions. Returns when the UI dropped the link.
async fn drain_until_closed(out: &mut mpsc::UnboundedReceiver<Vec<u8>>) {
    while out.recv().await.is_some() {}
}

enum Ending {
    /// The UI dropped the link.
    Dropped,
    Refused,
    /// Why, and whether a hello had been accepted first.
    Lost(String, bool),
}

/// Why a socket failed, in the same few words on every platform: Windows
/// spells an OS error as a sentence with its number ("No connection could be
/// made because the target machine actively refused it. (os error 10061)").
fn why(e: &tokio_tungstenite::tungstenite::Error) -> String {
    use std::io::ErrorKind::*;
    use tokio_tungstenite::tungstenite::Error;
    use tokio_tungstenite::tungstenite::error::ProtocolError;
    match e {
        Error::Io(io) => match io.kind() {
            ConnectionRefused => "refused".into(),
            ConnectionReset | ConnectionAborted | BrokenPipe | UnexpectedEof => {
                "the connection dropped".into()
            }
            TimedOut => "timed out".into(),
            NotFound | AddrNotAvailable | HostUnreachable | NetworkUnreachable => {
                "no route to it".into()
            }
            _ => io.to_string(),
        },
        Error::ConnectionClosed
        | Error::AlreadyClosed
        | Error::Protocol(ProtocolError::ResetWithoutClosingHandshake) => {
            "the connection dropped".into()
        }
        e => e.to_string(),
    }
}

async fn session(
    id: LinkId,
    url: &str,
    out: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    events: &async_channel::Sender<Event>,
) -> Ending {
    let ws = match tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(url),
    )
    .await
    {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => return Ending::Lost(why(&e), false),
        Err(_) => return Ending::Lost("no answer in 5 s".into(), false),
    };
    if let tokio_tungstenite::MaybeTlsStream::Plain(s) = ws.get_ref() {
        // Keystrokes go out at once.
        let _ = s.set_nodelay(true);
    }
    let (mut tx, mut rx) = ws.split();
    // Anything queued before this socket belongs to the last one.
    while out.try_recv().is_ok() {}
    let mut open = false;
    loop {
        tokio::select! {
            frame = out.recv() => {
                let Some(frame) = frame else { return Ending::Dropped };
                if open && tx.send(Message::Binary(frame.into())).await.is_err() {
                    return Ending::Lost("the socket closed".into(), open);
                }
            }
            msg = rx.next() => {
                let bytes = match msg {
                    Some(Ok(Message::Binary(b))) => b,
                    Some(Ok(Message::Close(_))) | None => return Ending::Lost("the node closed the socket".into(), open),
                    Some(Err(e)) => return Ending::Lost(why(&e), open),
                    Some(Ok(_)) => continue,
                };
                if !open {
                    match frames::hello(&bytes) {
                        Some(hello) if hello.protocol == PROTOCOL => {
                            open = true;
                            if events.send(Event::Opened(id, hello)).await.is_err() {
                                return Ending::Dropped;
                            }
                        }
                        hello => {
                            let _ = events.send(Event::Refused(id, refusal(hello.as_ref()))).await;
                            return Ending::Refused;
                        }
                    }
                    continue;
                }
                let Some((&kind, body)) = bytes.split_first() else { continue };
                if events.send(Event::Frame(id, kind, body.to_vec())).await.is_err() {
                    return Ending::Dropped;
                }
            }
        }
    }
}

/// Why this client cannot talk to the node that sent `hello`.
pub fn refusal(hello: Option<&Hello>) -> String {
    let Some(h) = hello else {
        return "The node did not say hello: it runs an older Parvi. Upgrade Parvi there.".into();
    };
    let at = format!("{} ({})", h.device, h.environment);
    if h.protocol > PROTOCOL {
        format!(
            "{at} runs Parvi {}, protocol {}; this app speaks protocol {PROTOCOL}. Upgrade this app.",
            h.version, h.protocol
        )
    } else {
        format!(
            "{at} runs Parvi {}, protocol {}; this app speaks protocol {PROTOCOL}. Upgrade Parvi on {}.",
            h.version, h.protocol, h.device
        )
    }
}

/// The `/ws` address of a node reached through the serving node at `base`
/// (`127.0.0.1:7300`, `http://host:port` or `ws://host:port`).
pub fn ws_url(base: &str, node: Option<&str>) -> String {
    let base = base.trim().trim_end_matches('/');
    let host = base
        .strip_prefix("http://")
        .or_else(|| base.strip_prefix("ws://"))
        .map(|h| format!("ws://{h}"))
        .or_else(|| {
            base.strip_prefix("https://")
                .or_else(|| base.strip_prefix("wss://"))
                .map(|h| format!("wss://{h}"))
        })
        .unwrap_or_else(|| format!("ws://{base}"));
    match node {
        Some(id) => format!("{host}/ws?node={id}"),
        None => format!("{host}/ws"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_thirty_seconds() {
        let secs: Vec<u64> = (0..8).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn a_failed_socket_says_why_in_words() {
        use tokio_tungstenite::tungstenite::Error;
        let io = |k| Error::Io(std::io::Error::from(k));
        assert_eq!(why(&io(std::io::ErrorKind::ConnectionRefused)), "refused");
        assert_eq!(
            why(&io(std::io::ErrorKind::ConnectionReset)),
            "the connection dropped"
        );
        let reset = Error::Protocol(
            tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
        );
        assert_eq!(why(&reset), "the connection dropped");
    }

    #[test]
    fn addresses_a_node_through_the_serving_node() {
        assert_eq!(ws_url("127.0.0.1:7300", None), "ws://127.0.0.1:7300/ws");
        assert_eq!(
            ws_url("http://rig:7300/", Some("ab")),
            "ws://rig:7300/ws?node=ab"
        );
        assert_eq!(
            ws_url("https://rig.tail.ts.net", None),
            "wss://rig.tail.ts.net/ws"
        );
    }

    #[test]
    fn a_newer_node_asks_to_upgrade_the_app() {
        let h = Hello {
            node: "n".into(),
            device: "rig".into(),
            environment: "wsl".into(),
            version: "9".into(),
            protocol: PROTOCOL + 1,
            off: vec![],
        };
        assert!(refusal(Some(&h)).contains("Upgrade this app"));
        let h = Hello {
            protocol: PROTOCOL - 1,
            ..h
        };
        assert!(refusal(Some(&h)).contains("Upgrade Parvi on rig"));
    }
}
