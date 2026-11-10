//! A connected control client.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::outbox::{LIMIT, Outbox};
use crate::reply::escape_body;

pub struct Client {
    pub id: u32,
    pub name: String,
    pub outbox: Outbox,
    pub flags: Mutex<Flags>,
}

#[derive(Default)]
pub struct Flags {
    /// The session this client is attached to.
    pub session: Option<u32>,
    /// Whether this client's size is left out of sizing its session.
    pub ignore_size: bool,
    /// The size this client last asked for (`refresh-client -C`).
    pub size: Option<(u16, u16)>,
    pub pause_after: Option<Duration>,
}

impl Client {
    pub fn new(id: u32) -> Self {
        Client {
            id,
            name: format!("holder-client-{id}"),
            outbox: Outbox::new(LIMIT),
            flags: Mutex::default(),
        }
    }

    /// Answers command `number`: its output between `%begin` and `%end`, or
    /// its error between `%begin` and `%error`.
    pub fn reply(&self, number: u64, result: Result<String, String>) {
        let time = now();
        let (ok, body) = match result {
            Ok(body) => (true, body),
            Err(body) => (false, body),
        };
        let mut out = format!("%begin {time} {number} 1\n");
        escape_body(&mut out, &body);
        if !body.is_empty() && !body.ends_with('\n') {
            out.push('\n');
        }
        let end = if ok { "end" } else { "error" };
        out.push_str(&format!("%{end} {time} {number} 1\n"));
        self.outbox.push(out.into_bytes());
    }

    /// `#{client_flags}`.
    pub fn flag_list(&self) -> String {
        let flags = self.flags.lock().unwrap();
        let mut list = vec!["control-mode"];
        if flags.session.is_some() {
            list.insert(0, "attached");
        }
        if flags.ignore_size {
            list.push("ignore-size");
        }
        let mut list = list.join(",");
        if let Some(after) = flags.pause_after {
            list.push_str(&format!(",pause-after={}", after.as_secs()));
        }
        list
    }
}

/// Seconds since the epoch, as tmux's times are.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
