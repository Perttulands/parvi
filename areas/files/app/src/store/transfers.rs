//! Transfers between devices as this app started them: what each moves,
//! from where to where, how far it got and how fast. The source node does
//! the work; this keeps the request so Resume can ask again.

use parvi_files_contract::{Progress, TransferRequest};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Running,
    /// The destination device is away; the transfer resumes when it answers.
    Waiting,
    Done,
    Failed,
}

/// A device and environment, as the transfer row names its ends.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct End {
    pub device: String,
    pub environment: String,
    /// The path or folder as the operator reads it (`~/kuvio/models`).
    pub shown: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub request: TransferRequest,
    pub name: String,
    pub from: End,
    pub to: End,
    pub state: State,
    pub bytes: u64,
    pub total: u64,
    #[serde(default)]
    pub error: Option<String>,
    /// When it finished, in Unix ms.
    #[serde(default)]
    pub finished: Option<u64>,
}

/// A transfer and the samples its rate is measured from.
#[derive(Clone, Debug)]
pub struct Transfer {
    pub kept: Kept,
    /// (ms, bytes), the last few seconds.
    samples: Vec<(u64, u64)>,
}

/// The window the rate is measured over.
const RATE_WINDOW_MS: u64 = 5000;

impl Transfer {
    pub fn new(kept: Kept) -> Self {
        Self {
            kept,
            samples: vec![],
        }
    }

    pub fn id(&self) -> &str {
        &self.kept.request.id
    }

    /// Progress from the source. Returns true when this ended it.
    pub fn progress(&mut self, p: &Progress, now: u64) -> bool {
        let was = self.kept.state;
        self.kept.bytes = p.bytes;
        self.kept.total = p.total;
        self.kept.state = match p.state.as_str() {
            "done" => State::Done,
            "failed" => State::Failed,
            _ => State::Running,
        };
        self.kept.error = p.error.clone();
        self.samples.push((now, p.bytes));
        self.samples
            .retain(|(t, _)| now.saturating_sub(*t) <= RATE_WINDOW_MS);
        let ended = was == State::Running && self.kept.state != State::Running;
        if ended {
            self.kept.finished = Some(now);
        }
        ended
    }

    /// Bytes per second over the last few seconds.
    pub fn rate(&self) -> Option<f64> {
        let (t0, b0) = *self.samples.first()?;
        let (t1, b1) = *self.samples.last()?;
        (t1 > t0 && b1 >= b0).then(|| (b1 - b0) as f64 * 1000. / (t1 - t0) as f64)
    }

    /// Seconds until done at the current rate.
    pub fn left(&self) -> Option<f64> {
        let r = self.rate().filter(|r| *r > 0.)?;
        (self.kept.total >= self.kept.bytes).then(|| (self.kept.total - self.kept.bytes) as f64 / r)
    }

    pub fn fraction(&self) -> f32 {
        if self.kept.total == 0 {
            0.
        } else {
            (self.kept.bytes as f32 / self.kept.total as f32).clamp(0., 1.)
        }
    }

    /// Resume starts again from what is staged.
    pub fn restart(&mut self) {
        self.kept.state = State::Running;
        self.kept.error = None;
        self.kept.finished = None;
        self.samples.clear();
    }

    pub fn wait(&mut self, why: &str) {
        self.kept.state = State::Waiting;
        self.kept.error = Some(why.to_string());
        self.samples.clear();
    }
}

/// A fresh transfer id: 32 hex digits, as the source requires.
pub fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept() -> Kept {
        Kept {
            request: TransferRequest {
                node: "a".into(),
                path: "/x/f".into(),
                target_node: "b".into(),
                folder: "C:\\in".into(),
                id: new_id(),
            },
            name: "f".into(),
            from: End::default(),
            to: End::default(),
            state: State::Running,
            bytes: 0,
            total: 0,
            error: None,
            finished: None,
        }
    }

    fn p(state: &str, bytes: u64) -> Progress {
        Progress {
            state: state.into(),
            bytes,
            total: 1000,
            error: None,
        }
    }

    #[test]
    fn rate_and_time_left_come_from_recent_progress() {
        let mut t = Transfer::new(kept());
        assert!(!t.progress(&p("running", 0), 1000));
        assert!(!t.progress(&p("running", 200), 2000));
        assert_eq!(t.rate(), Some(200.));
        assert_eq!(t.left(), Some(4.));
        assert!(t.progress(&p("done", 1000), 3000));
        assert_eq!(t.kept.state, State::Done);
        assert_eq!(t.kept.finished, Some(3000));
        assert_eq!(t.fraction(), 1.);
    }

    #[test]
    fn ids_are_32_hex_digits_and_differ() {
        let (a, b) = (new_id(), new_id());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
