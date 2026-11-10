//! A pane's input on its way to the program, bounded.
//!
//! Writes to a console block while the program isn't reading, so they happen
//! on a writer thread of the pane's own. What waits for it is capped: once a
//! program stops reading and the cap is reached, more input is refused, and
//! the command that brought it fails, instead of the holder growing until it
//! dies with every session.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

/// Bytes of input a pane may hold unwritten.
pub const LIMIT: usize = 1 << 20;

pub struct Input {
    limit: usize,
    state: Mutex<State>,
    ready: Condvar,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Vec<u8>>,
    /// Queued plus being written.
    bytes: usize,
    closed: bool,
}

impl Input {
    pub fn new(limit: usize) -> Self {
        Input {
            limit,
            state: Mutex::default(),
            ready: Condvar::new(),
        }
    }

    /// Queues bytes for the program, unless the session ended or too much
    /// is waiting already.
    pub fn push(&self, bytes: Vec<u8>) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err("session ended".into());
        }
        if state.bytes + bytes.len() > self.limit {
            return Err(format!(
                "input refused: {} bytes wait for a program that isn't reading",
                state.bytes
            ));
        }
        state.bytes += bytes.len();
        state.queue.push_back(bytes);
        self.ready.notify_one();
        Ok(())
    }

    /// The next bytes to write, waiting for some; `None` once closed. Call
    /// [`Input::written`] after writing them.
    pub fn next(&self) -> Option<Vec<u8>> {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.closed {
                return None;
            }
            if let Some(bytes) = state.queue.pop_front() {
                return Some(bytes);
            }
            state = self.ready.wait(state).unwrap();
        }
    }

    pub fn written(&self, len: usize) {
        self.state.lock().unwrap().bytes -= len;
    }

    /// Ends the input: what waits is dropped, and the writer stops.
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.queue.clear();
        self.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_beyond_the_limit_is_refused_until_written() {
        let input = Input::new(10);
        input.push(b"0123456".to_vec()).unwrap();
        let refused = input.push(b"89ab".to_vec()).unwrap_err();
        assert!(refused.contains("7 bytes wait"), "{refused}");
        let taken = input.next().unwrap();
        // Still counted while it is being written.
        assert!(input.push(b"89ab".to_vec()).is_err());
        input.written(taken.len());
        input.push(b"89ab".to_vec()).unwrap();
        assert!(input.push(vec![0; 11]).is_err());
    }

    #[test]
    fn a_closed_input_refuses_and_stops_its_writer() {
        let input = std::sync::Arc::new(Input::new(10));
        let writer = {
            let input = input.clone();
            std::thread::spawn(move || input.next())
        };
        input.close();
        assert_eq!(writer.join().unwrap(), None);
        assert_eq!(input.push(b"x".to_vec()).unwrap_err(), "session ended");
    }
}
