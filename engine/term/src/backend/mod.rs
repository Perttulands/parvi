//! Session backends speak tmux's command and control language. A backend owns
//! command execution; callers never need to know whether it is a process or a
//! named pipe.

mod client;
#[cfg(windows)]
pub mod holder;
pub mod tmux;
#[cfg(unix)]
mod tmux_socket;

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;

pub use client::{ControlClient, Reader, Writer};

pub type Reply<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
pub type Attaching<'a> = Reply<'a, ControlClient>;

/// A command's output and completion. Dropping either cancels its producer.
pub struct Stream {
    pub reader: Reader,
    pub completion: tokio::task::JoinHandle<Result<()>>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.completion.abort();
    }
}

/// One server of sessions, such as one tmux socket or holder.
pub trait Backend: Send + Sync + 'static {
    /// Shell command for a tmux pane to wait for its engine; absent on other backends.
    fn launch_gate(&self, _channel: &str) -> Option<String> {
        None
    }

    /// Start an owned session at a fixed size.
    fn create_session<'a>(&'a self, spec: &'a crate::SessionSpec) -> Reply<'a, crate::SessionRef>;

    /// Kill only if the server generation still matches.
    fn kill_session<'a>(&'a self, session: &'a crate::SessionRef) -> Reply<'a, ()>;

    /// Run a command and collect its reply.
    fn command<'a>(&'a self, args: &'a [&'a str]) -> Reply<'a, String>;

    /// Set a new session's initial size without pinning it.
    fn size<'a>(&'a self, session: &'a str, cols: u16, rows: u16) -> Reply<'a, ()>;

    /// Stream a command's output, reporting errors separately at completion.
    fn stream<'a>(&'a self, args: &'a [&'a str]) -> Reply<'a, Stream>;

    /// Attach without taking part in sizing, pausing panes that fall behind.
    fn attach<'a>(&'a self, session: &'a str) -> Attaching<'a>;
}
