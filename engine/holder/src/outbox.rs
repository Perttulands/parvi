//! One client's outgoing bytes, and the flow control that keeps them bounded.
//!
//! Producers (a pane's reader thread, command replies) never wait for a
//! client: a client that stops reading cannot stall the holder. Instead its
//! queue is bounded. A client attached with `pause-after` has a pane paused
//! once that pane's output would overfill its queue or has waited longer than
//! `pause-after`: the holder drops the pane's output for it and sends
//! `%pause %<pane>`, as tmux does, until the client asks to continue. Any other
//! client that overfills is told `%exit too far behind` and dropped, and so is
//! any client that lets replies pile up past twice the limit.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::Notify;

/// Queued bytes a client may fall behind by.
pub const LIMIT: usize = 1 << 20;

pub struct Outbox {
    limit: usize,
    state: Mutex<State>,
    ready: Notify,
    dropped: Notify,
}

#[derive(Default)]
struct State {
    queue: VecDeque<(Instant, Vec<u8>)>,
    bytes: usize,
    pause_after: Option<Duration>,
    paused: BTreeSet<u32>,
    closed: bool,
}

impl Outbox {
    pub fn new(limit: usize) -> Self {
        Outbox {
            limit,
            state: Mutex::default(),
            ready: Notify::new(),
            dropped: Notify::new(),
        }
    }

    /// Queues a reply or notification. These are never paused, so they get
    /// a second limit of room above where output pauses. A client past that
    /// (pipelining big replies it doesn't read) is too far behind. A reply
    /// to a client that has caught up is always queued, however big.
    pub fn push(&self, bytes: Vec<u8>) {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return;
        }
        if state.bytes > 0 && state.bytes + bytes.len() > 2 * self.limit {
            self.drop_behind(&mut state);
        } else {
            state.enqueue(bytes);
        }
        self.ready.notify_one();
    }

    /// Queues a pane's output line, unless flow control says otherwise.
    pub fn output(&self, pane: u32, line: &[u8]) {
        let mut state = self.state.lock().unwrap();
        if state.closed || state.paused.contains(&pane) {
            return;
        }
        let full = state.bytes + line.len() > self.limit;
        match state.pause_after {
            Some(after) => {
                let late = state
                    .queue
                    .front()
                    .is_some_and(|(at, _)| at.elapsed() > after);
                if full || late {
                    state.paused.insert(pane);
                    state.enqueue(format!("%pause %{pane}\n").into_bytes());
                } else {
                    state.enqueue(line.to_vec());
                }
            }
            None if full => self.drop_behind(&mut state),
            None => state.enqueue(line.to_vec()),
        }
        self.ready.notify_one();
    }

    /// Drops the client: what it has not read is discarded for one last
    /// line.
    fn drop_behind(&self, state: &mut State) {
        state.queue.clear();
        state.bytes = 0;
        state.enqueue(b"%exit too far behind\n".to_vec());
        state.closed = true;
        self.dropped.notify_one();
    }

    /// `pause-after=<seconds>` from the client's attach or refresh flags.
    pub fn set_pause_after(&self, after: Option<Duration>) {
        let mut state = self.state.lock().unwrap();
        state.pause_after = after;
        if after.is_none() {
            state.paused.clear();
        }
    }

    /// Continues a paused pane: `refresh-client -A '%<pane>:continue'`.
    pub fn resume(&self, pane: u32) {
        let mut state = self.state.lock().unwrap();
        if state.paused.remove(&pane) {
            state.enqueue(format!("%continue %{pane}\n").into_bytes());
            self.ready.notify_one();
        }
    }

    /// Ends the client: [`Outbox::next`] gives what is queued, then `None`.
    pub fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.ready.notify_one();
    }

    /// Resolves once the client is dropped as too far behind. Its writer may
    /// be stuck on a pipe nobody reads, so the connection ends here instead.
    pub async fn dropped(&self) {
        self.dropped.notified().await;
    }

    /// Everything queued so far, once there is something; `None` once closed
    /// and drained.
    pub async fn next(&self) -> Option<Vec<u8>> {
        loop {
            {
                let mut state = self.state.lock().unwrap();
                if !state.queue.is_empty() {
                    let mut batch = Vec::with_capacity(state.bytes);
                    for (_, bytes) in state.queue.drain(..) {
                        batch.extend_from_slice(&bytes);
                    }
                    state.bytes = 0;
                    return Some(batch);
                }
                if state.closed {
                    return None;
                }
            }
            self.ready.notified().await;
        }
    }
}

impl State {
    fn enqueue(&mut self, bytes: Vec<u8>) {
        self.bytes += bytes.len();
        self.queue.push_back((Instant::now(), bytes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: Option<Vec<u8>>) -> String {
        String::from_utf8(bytes.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn a_reader_gets_everything_in_order() {
        let outbox = Outbox::new(100);
        outbox.push(b"%begin\n".to_vec());
        outbox.output(1, b"%output %1 a\n");
        assert_eq!(text(outbox.next().await), "%begin\n%output %1 a\n");
        outbox.close();
        assert_eq!(outbox.next().await, None);
    }

    #[tokio::test]
    async fn a_full_queue_pauses_the_pane_until_continued() {
        let outbox = Outbox::new(30);
        outbox.set_pause_after(Some(Duration::from_secs(1)));
        outbox.output(1, b"%output %1 0123456\n");
        outbox.output(1, b"%output %1 overflow\n");
        outbox.output(1, b"%output %1 dropped\n");
        outbox.output(2, b"%output %2 x\n");
        assert_eq!(
            text(outbox.next().await),
            "%output %1 0123456\n%pause %1\n%pause %2\n"
        );
        outbox.output(2, b"%output %2 y\n");
        outbox.resume(1);
        outbox.output(1, b"%output %1 back\n");
        assert_eq!(text(outbox.next().await), "%continue %1\n%output %1 back\n");
    }

    #[tokio::test]
    async fn replies_a_client_does_not_read_are_bounded() {
        let outbox = Outbox::new(20);
        outbox.set_pause_after(Some(Duration::from_secs(1)));
        // However big, a reply to a client that has read everything goes.
        outbox.push(vec![b'x'; 100]);
        assert_eq!(outbox.next().await.unwrap().len(), 100);
        for _ in 0..4 {
            outbox.push(b"%begin\n0123\n%end\n".to_vec());
        }
        outbox.dropped().await;
        assert_eq!(text(outbox.next().await), "%exit too far behind\n");
        assert_eq!(outbox.next().await, None);
    }

    #[tokio::test]
    async fn late_output_pauses_too() {
        let outbox = Outbox::new(1000);
        outbox.set_pause_after(Some(Duration::ZERO));
        outbox.output(1, b"%output %1 a\n");
        std::thread::sleep(Duration::from_millis(2));
        outbox.output(1, b"%output %1 b\n");
        assert_eq!(text(outbox.next().await), "%output %1 a\n%pause %1\n");
    }

    #[tokio::test]
    async fn without_pause_after_a_full_client_is_dropped() {
        let outbox = Outbox::new(20);
        outbox.output(1, b"%output %1 0123456\n");
        outbox.output(1, b"%output %1 overflow\n");
        outbox.push(b"%begin\n".to_vec());
        outbox.dropped().await;
        assert_eq!(text(outbox.next().await), "%exit too far behind\n");
        assert_eq!(outbox.next().await, None);
    }
}
