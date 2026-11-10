//! Runtime events: what happened to this node that an operator would want
//! to know when something looks degraded (the node started, a peer went
//! away or came back, a service action, a transfer that failed). One
//! bounded ring per node, [`KEPT`] long, in memory only. Recording is a push
//! and a send nobody may hear; an area that shows them carries the ring and
//! each new event to whoever has it on, so an idle node does no work for it.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast;

use crate::ring::Ring;

/// Events kept; older ones fall off.
pub const KEPT: usize = 200;

/// The node lived or stopped. Every other kind is a plain string the area
/// that records it owns.
pub const NODE: &str = "node";

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Milliseconds since the epoch.
    pub at: u64,
    pub kind: &'static str,
    /// Whether it is a failure rather than news.
    pub failure: bool,
    pub text: String,
}

pub struct Log {
    ring: Mutex<Ring<Event>>,
    sent: broadcast::Sender<Event>,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            ring: Mutex::new(Ring::new(KEPT)),
            sent: broadcast::channel(16).0,
        }
    }
}

impl Log {
    /// Keeps an event and tells whoever listens.
    pub fn record(&self, kind: &'static str, failure: bool, text: impl Into<String>) {
        let event = Event {
            at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            kind,
            failure,
            text: text.into(),
        };
        let mut ring = self.ring.lock().unwrap();
        ring.push(event.clone());
        // Sent under the lock, so a listener's snapshot and its stream meet
        // without a gap or a repeat.
        let _ = self.sent.send(event);
    }

    /// The events kept, oldest first, and every event after them.
    pub fn listen(&self) -> (Vec<Event>, broadcast::Receiver<Event>) {
        let ring = self.ring.lock().unwrap();
        (ring.iter().cloned().collect(), self.sent.subscribe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_is_bounded_and_a_listener_misses_nothing() {
        let log = Log::default();
        let (before, mut after) = log.listen();
        for i in 0..KEPT + 5 {
            log.record(NODE, false, format!("test {i}"));
        }
        let (kept, _) = log.listen();
        assert_eq!(kept.len(), KEPT, "{} before", before.len());
        let last = format!("test {}", KEPT + 4);
        assert!(kept.iter().any(|e| e.text == last));
        assert!(
            !kept.iter().any(|e| e.text == "test 0"),
            "the oldest fell off"
        );
        // The listener lags past its buffer; it learns so and re-reads.
        assert!(matches!(
            after.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
    }
}
