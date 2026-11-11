//! Host: what this node's machine is doing (ARCHITECTURE.md, Host).
//!
//! One sampler per node reads CPU, memory, swap, load, disk, GPUs and
//! uptime every [`EVERY`] into a day of history: the last [`HISTORY`]
//! points (an hour) as read, minutes before them (`history.rs`). It is the
//! one standing timer Parvi keeps without a subscriber, so the Fleet tab
//! opens on the day so far. A browser that turns the host channel on gets
//! the whole state with the node's runtime events (`crate::events`), then
//! each reading as it is taken and each event as it happens; one that turns
//! it off, or goes, costs nothing more.
//!
//! Linux, WSL and Windows. On WSL the figures are the VM's, and Fleet uses
//! the hello environment to label them; Windows has no load average, and
//! its swap is the commit charge.

mod channel;
mod cpu;
mod disk;
mod facts;
mod gpu;
mod history;
#[cfg(not(windows))]
mod procfs;
mod reading;
mod ring;
pub mod services;
#[cfg(windows)]
mod windows;

#[cfg(not(windows))]
use procfs as os;
#[cfg(windows)]
use windows as os;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use facts::Facts;
pub use history::History;

pub use channel::{HOST, HOST_EVENT, HOST_READING, HOST_STATE, decode_on, encode_on, owns};
pub use reading::{Point, Probe, Reading};
pub use ring::Ring;

/// How often the sampler reads the host.
pub const EVERY: Duration = Duration::from_secs(5);

/// Points kept as read: an hour at [`EVERY`].
pub const HISTORY: usize = 720;

/// How far back the history reaches.
pub const DAY: Duration = Duration::from_secs(24 * 3600);

/// The node's host: its facts, its history and the sampler keeping it.
/// Dropping it stops the sampler.
pub struct Host {
    shared: Arc<Shared>,
    sampler: JoinHandle<()>,
}

struct Shared {
    events: Arc<parvi_node_kit::events::Log>,
    facts: Facts,
    every: Duration,
    kept: Mutex<Kept>,
    /// Each reading's frame, for the browsers streaming.
    readings: broadcast::Sender<Arc<Vec<u8>>>,
}

struct Kept {
    history: History,
    latest: Option<Reading>,
}

impl Host {
    /// Reads the facts and starts sampling every `every`, keeping `recent`
    /// points as read and minutes before them to make up a [`DAY`], one of
    /// them still filling. The first reading is taken at once.
    pub fn start(every: Duration, recent: usize, events: Arc<parvi_node_kit::events::Log>) -> Host {
        let shared = Arc::new(Shared {
            events,
            facts: facts::read(),
            every,
            kept: Mutex::new(Kept {
                history: History::spanning(DAY, every, recent),
                latest: None,
            }),
            readings: broadcast::channel(4).0,
        });
        let sampler = tokio::spawn(sample(shared.clone()));
        Host { shared, sampler }
    }

    /// Sends the state frame, then a reading frame for each reading, until
    /// the browser's outbox closes. A browser too slow to keep up gets the
    /// state again instead of what it missed.
    pub fn stream(&self, outbox: mpsc::Sender<Vec<u8>>) -> impl Future<Output = ()> + use<> {
        let shared = self.shared.clone();
        async move {
            let (state, mut readings, mut events) = shared.state_and_stream();
            if outbox.send(state).await.is_err() {
                return;
            }
            loop {
                let lagged = tokio::select! {
                    reading = readings.recv() => match reading {
                        Ok(frame) => Ok(frame.as_ref().clone()),
                        Err(broadcast::error::RecvError::Lagged(_)) => Err(()),
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                    event = events.recv() => match event {
                        Ok(event) => Ok(channel::event(&event)),
                        Err(broadcast::error::RecvError::Lagged(_)) => Err(()),
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                };
                let frame = match lagged {
                    Ok(frame) => frame,
                    Err(()) => {
                        let (state, r, e) = shared.state_and_stream();
                        (readings, events) = (r, e);
                        state
                    }
                };
                if outbox.send(frame).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// The host now, as the Host view shows it: its facts, a reading taken
/// over `window` (CPU is busy time across it), and the recent events.
pub async fn now(window: Duration, log: &parvi_node_kit::events::Log) -> serde_json::Value {
    let mut probe = reading::Probe::new();
    tokio::time::sleep(window).await;
    let reading = probe.read().await;
    let facts = facts::read();
    let (events, _) = log.listen();
    let frame = channel::state(&channel::State {
        facts: &facts,
        every_ms: 0,
        history: &History::new(1, 1),
        latest: Some(&reading),
        events: &events,
    });
    let mut state: serde_json::Value =
        serde_json::from_slice(&frame[1..]).expect("the state frame is JSON");
    if let Some(state) = state.as_object_mut() {
        for key in ["history", "earlier", "bucket", "every"] {
            state.remove(key);
        }
    }
    state
}

impl Drop for Host {
    fn drop(&mut self) {
        self.sampler.abort();
    }
}

impl Shared {
    /// The state frame and receivers of every reading and event after it,
    /// taken under the locks so nothing falls between them.
    fn state_and_stream(
        &self,
    ) -> (
        Vec<u8>,
        broadcast::Receiver<Arc<Vec<u8>>>,
        broadcast::Receiver<parvi_node_kit::events::Event>,
    ) {
        let kept = self.kept.lock().unwrap();
        let (logged, events) = self.events.listen();
        let state = channel::state(&channel::State {
            facts: &self.facts,
            every_ms: self.every.as_millis() as u64,
            history: &kept.history,
            latest: kept.latest.as_ref(),
            events: &logged,
        });
        (state, self.readings.subscribe(), events)
    }
}

/// The sampler: one reading per tick, into history and out to every stream.
async fn sample(shared: Arc<Shared>) {
    let mut probe = Probe::new();
    let mut tick = tokio::time::interval(shared.every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let reading = probe.read().await;
        let point = reading::point_of(&reading, shared.facts.threads);
        let frame = Arc::new(channel::reading(&reading, &point));
        let mut kept = shared.kept.lock().unwrap();
        kept.history.push(point);
        kept.latest = Some(reading);
        let _ = shared.readings.send(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publish(shared: &Shared, at: u64) {
        let reading = Reading {
            at,
            cpu: None,
            memory: None,
            swap: None,
            load: None,
            disk: None,
            gpus: Vec::new(),
            uptime: None,
        };
        let point = reading::point_of(&reading, 1);
        let mut kept = shared.kept.lock().unwrap();
        kept.history.push(point);
        kept.latest = Some(reading.clone());
        let _ = shared
            .readings
            .send(Arc::new(channel::reading(&reading, &point)));
    }

    fn json(frame: &[u8]) -> serde_json::Value {
        serde_json::from_slice(&frame[1..]).unwrap()
    }

    #[tokio::test]
    async fn a_lagged_stream_resumes_after_its_recovery_state() {
        let host = Host::start(Duration::from_secs(60), 16, Arc::default());
        host.sampler.abort();
        let (outbox, mut frames) = mpsc::channel(1);
        let streaming = tokio::spawn(host.stream(outbox));
        assert_eq!(frames.recv().await.unwrap()[0], HOST_STATE);

        publish(&host.shared, 1);
        tokio::task::yield_now().await;
        publish(&host.shared, 2);
        tokio::task::yield_now().await;
        for at in 3..=10 {
            publish(&host.shared, at);
        }
        assert_eq!(json(&frames.recv().await.unwrap())["reading"]["at"], 1);
        assert_eq!(json(&frames.recv().await.unwrap())["reading"]["at"], 2);
        let state = frames.recv().await.unwrap();
        assert_eq!(state[0], HOST_STATE);
        let snapshot_at = json(&state)["latest"]["at"].as_u64().unwrap();
        assert_eq!(snapshot_at, 10);

        for at in 11..=13 {
            publish(&host.shared, at);
        }
        for expected in 11..=13 {
            let frame = frames.recv().await.unwrap();
            assert_eq!(frame[0], HOST_READING);
            let at = json(&frame)["reading"]["at"].as_u64().unwrap();
            assert!(at > snapshot_at);
            assert_eq!(at, expected);
        }
        drop(frames);
        streaming.abort();
    }

    #[tokio::test]
    async fn node_streams_keep_snapshots_and_live_events_isolated() {
        let first_log = Arc::new(parvi_node_kit::events::Log::default());
        let second_log = Arc::new(parvi_node_kit::events::Log::default());
        first_log.record(parvi_node_kit::events::NODE, false, "first before");
        second_log.record(parvi_node_kit::events::NODE, false, "second before");
        let first = Host::start(Duration::from_secs(60), 4, first_log.clone());
        let second = Host::start(Duration::from_secs(60), 4, second_log.clone());
        first.sampler.abort();
        second.sampler.abort();
        let (first_tx, mut first_rx) = mpsc::channel(8);
        let (second_tx, mut second_rx) = mpsc::channel(8);
        let a = tokio::spawn(first.stream(first_tx));
        let b = tokio::spawn(second.stream(second_tx));
        let first_state = json(&first_rx.recv().await.unwrap()).to_string();
        let second_state = json(&second_rx.recv().await.unwrap()).to_string();
        assert!(first_state.contains("first before") && !first_state.contains("second before"));
        assert!(second_state.contains("second before") && !second_state.contains("first before"));
        first_log.record("service", true, "first after");
        second_log.record("service", true, "second after");
        let first_event = first_rx.recv().await.unwrap();
        let second_event = second_rx.recv().await.unwrap();
        assert_eq!(first_event[0], HOST_EVENT);
        assert_eq!(second_event[0], HOST_EVENT);
        assert!(json(&first_event).to_string().contains("first after"));
        assert!(json(&second_event).to_string().contains("second after"));
        assert!(first_rx.try_recv().is_err());
        assert!(second_rx.try_recv().is_err());
        a.abort();
        b.abort();
    }

    #[tokio::test]
    async fn a_stream_starts_with_the_state_then_follows_each_reading() {
        let host = Host::start(Duration::from_millis(50), 4, Arc::default());
        let (outbox, mut frames) = mpsc::channel(8);
        let streaming = tokio::spawn(host.stream(outbox));
        let first = frames.recv().await.unwrap();
        assert_eq!(first[0], HOST_STATE);
        for _ in 0..2 {
            let next = frames.recv().await.unwrap();
            assert_eq!(next[0], HOST_READING);
        }
        drop(frames);
        tokio::time::timeout(Duration::from_secs(5), streaming)
            .await
            .expect("the stream ends when its browser goes")
            .unwrap();
    }
}
