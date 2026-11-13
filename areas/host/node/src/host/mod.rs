//! Host: what this node's machine is doing.
//!
//! One sampler per node keeps a day of the host in minutes (`history.rs`).
//! While nobody looks it reads the host once every [`IDLE`]: `/proc`, a
//! `statvfs` per disk and one `nvidia-smi`, so the page opens on the day so
//! far. While an app has the host channel on it reads every [`EVERY`], and
//! what is heavy (every process's CPU time and memory) besides; the first
//! reading after someone starts to look comes at once and the next one
//! [`SOON`] after, so what is busy shows without waiting. A browser that
//! turns the channel on gets the whole state with the node's runtime events
//! (`crate::events`), then each reading and each event as it happens.
//!
//! Linux, WSL and Windows. On WSL the figures are the VM's, and its Windows
//! drives are reported beside the root; Windows reads no processes, and its
//! swap is the commit charge.

mod channel;
mod cpu;
mod disk;
mod facts;
mod gpu;
mod heavy;
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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{Notify, broadcast, mpsc};
use tokio::task::JoinHandle;

use facts::Facts;
pub use history::History;

pub use channel::{HOST, HOST_EVENT, HOST_READING, HOST_STATE, decode_on, encode_on, owns};
pub use reading::{Probe, Reading};
pub use ring::Ring;

/// How often the sampler reads the host while someone looks.
pub const EVERY: Duration = Duration::from_secs(5);
/// How often it reads while nobody looks: once a minute, the history's step.
pub const IDLE: Duration = Duration::from_secs(60);
/// The reading after the one taken when someone starts to look: soon
/// enough that what is busy shows at once, long enough to measure.
pub const SOON: Duration = Duration::from_secs(1);

/// The node's host: its facts, its history and the sampler keeping it.
/// Dropping it stops the sampler.
pub struct Host {
    shared: Arc<Shared>,
    sampler: JoinHandle<()>,
}

struct Shared {
    events: Arc<parvi_node_kit::events::Log>,
    facts: Facts,
    kept: Mutex<Kept>,
    /// Each reading's frame, for the browsers streaming.
    readings: broadcast::Sender<Arc<Vec<u8>>>,
    /// Streams open now.
    watchers: AtomicUsize,
    /// Rung when the first stream opens.
    looked: Notify,
}

struct Kept {
    history: History,
    latest: Option<Reading>,
}

/// One stream open: counted while it lives.
struct Watching(Arc<Shared>);

impl Drop for Watching {
    fn drop(&mut self) {
        self.0.watchers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Host {
    /// Reads the facts and starts sampling, every `idle` while nobody looks
    /// and every `every` while someone does, keeping a day of minutes. The
    /// first reading is taken at once.
    pub fn start(
        every: Duration,
        idle: Duration,
        events: Arc<parvi_node_kit::events::Log>,
    ) -> Host {
        let shared = Arc::new(Shared {
            events,
            facts: facts::read(),
            kept: Mutex::new(Kept {
                history: History::new(history::MINUTES),
                latest: None,
            }),
            readings: broadcast::channel(4).0,
            watchers: AtomicUsize::new(0),
            looked: Notify::new(),
        });
        let sampler = tokio::spawn(sample(shared.clone(), every, idle));
        Host { shared, sampler }
    }

    /// Sends the state frame, then a reading frame for each reading, until
    /// the browser's outbox closes. A browser too slow to keep up gets the
    /// state again instead of what it missed.
    pub fn stream(&self, outbox: mpsc::Sender<Vec<u8>>) -> impl Future<Output = ()> + use<> {
        let shared = self.shared.clone();
        async move {
            let (state, mut readings, mut events) = shared.state_and_stream();
            if shared.watchers.fetch_add(1, Ordering::SeqCst) == 0 {
                shared.looked.notify_one();
            }
            let _watching = Watching(shared.clone());
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

/// The host now, as `parvi host show` tells it: its facts, a reading taken
/// over `window` (CPU, and what is heavy, measured across it), and the
/// recent events.
pub async fn now(window: Duration, log: &parvi_node_kit::events::Log) -> serde_json::Value {
    let facts = facts::read();
    let mut probe = reading::Probe::new(disk::mounts(), facts.threads);
    probe.read(true).await;
    tokio::time::sleep(window).await;
    let reading = probe.read(true).await;
    let (events, _) = log.listen();
    let frame = channel::state(&facts, Vec::new(), Some(&reading), &events);
    let mut state: serde_json::Value =
        serde_json::from_slice(&frame[1..]).expect("the state frame is JSON");
    if let Some(state) = state.as_object_mut() {
        state.remove("minutes");
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
        let state = channel::state(
            &self.facts,
            kept.history.minutes(),
            kept.latest.as_ref(),
            &logged,
        );
        (state, self.readings.subscribe(), events)
    }

    fn watched(&self) -> bool {
        self.watchers.load(Ordering::SeqCst) > 0
    }
}

/// The sampler: one reading per tick, into history and out to every stream.
/// It waits `idle` between readings while nobody looks and `every` while
/// someone does; someone starting to look wakes it at once.
async fn sample(shared: Arc<Shared>, every: Duration, idle: Duration) {
    let mut probe = Probe::new(disk::mounts(), shared.facts.threads);
    let mut killed = os::oom_kills();
    let mut wait = Duration::ZERO;
    loop {
        let woken = tokio::select! {
            () = tokio::time::sleep(wait) => false,
            () = shared.looked.notified() => true,
        };
        let watched = shared.watched();
        let reading = probe.read(watched).await;
        let now_killed = os::oom_kills();
        if let (Some(before), Some(now)) = (killed, now_killed)
            && now > before
        {
            let n = now - before;
            shared.events.record(
                "host",
                true,
                format!(
                    "Out of memory: the kernel killed {} ({now} since boot)",
                    if n == 1 {
                        "a process".to_owned()
                    } else {
                        format!("{n} processes")
                    }
                ),
            );
        }
        killed = now_killed.or(killed);
        let frame = Arc::new(channel::reading(&reading));
        {
            let mut kept = shared.kept.lock().unwrap();
            kept.history.push(&parvi_host_contract::Point::of(&reading));
            kept.latest = Some(reading);
        }
        let _ = shared.readings.send(frame);
        wait = match (watched, woken) {
            (false, _) => idle,
            (true, true) => SOON.min(every),
            (true, false) => every,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publish(shared: &Shared, at: u64) {
        let reading = Reading {
            at,
            ..Reading::default()
        };
        let mut kept = shared.kept.lock().unwrap();
        kept.history.push(&parvi_host_contract::Point::of(&reading));
        kept.latest = Some(reading.clone());
        let _ = shared.readings.send(Arc::new(channel::reading(&reading)));
    }

    fn json(frame: &[u8]) -> serde_json::Value {
        serde_json::from_slice(&frame[1..]).unwrap()
    }

    #[tokio::test]
    async fn a_lagged_stream_resumes_after_its_recovery_state() {
        let host = Host::start(
            Duration::from_secs(60),
            Duration::from_secs(60),
            Arc::default(),
        );
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
        let first = Host::start(
            Duration::from_secs(60),
            Duration::from_secs(60),
            first_log.clone(),
        );
        let second = Host::start(
            Duration::from_secs(60),
            Duration::from_secs(60),
            second_log.clone(),
        );
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
        let host = Host::start(
            Duration::from_millis(50),
            Duration::from_secs(3600),
            Arc::default(),
        );
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

    #[tokio::test]
    async fn nobody_looking_costs_one_reading_an_idle_tick_and_no_processes() {
        let host = Host::start(
            Duration::from_millis(50),
            Duration::from_secs(3600),
            Arc::default(),
        );
        // The first reading is taken at once, without what is heavy.
        let first = loop {
            if let Some(r) = host.shared.kept.lock().unwrap().latest.clone() {
                break r;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(first.heavy.is_empty());
        let mut readings = host.shared.readings.subscribe();
        // Nothing more for as long as nobody looks.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            readings.try_recv().is_err(),
            "no reading while nobody looks"
        );
        // Someone looks: a reading at once, with the processes on Linux.
        let (outbox, mut frames) = mpsc::channel(8);
        let streaming = tokio::spawn(host.stream(outbox));
        assert_eq!(frames.recv().await.unwrap()[0], HOST_STATE);
        let looked = tokio::time::timeout(Duration::from_secs(2), frames.recv())
            .await
            .expect("a reading as soon as someone looks")
            .unwrap();
        assert_eq!(looked[0], HOST_READING);
        if cfg!(target_os = "linux") {
            assert!(
                !json(&looked)["reading"]["heavy"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
        // They go: the count falls back and the sampler idles again.
        streaming.abort();
        let _ = streaming.await;
        assert!(!host.shared.watched());
    }
}
