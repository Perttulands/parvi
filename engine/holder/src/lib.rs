//! Parvi's Windows session holder.
//!
//! A detached per-user process that owns ConPTY sessions and plays tmux's
//! part on Windows: a node attaches to it over a named pipe, and sessions
//! survive the node's restarts, upgrades and crashes. It speaks the subset of
//! tmux control mode that Parvi's engine uses, after a versioned greeting
//! ([`handshake`]).
//!
//! The holder runs on Windows; the wire format and the parts of it that are
//! plain logic build and are tested everywhere.

pub mod handshake;

// The holder's plain logic. Only the Windows server uses it, and it builds
// everywhere so its tests run in the Linux checks too.
#[cfg_attr(not(windows), allow(dead_code))]
mod format;
#[cfg_attr(not(windows), allow(dead_code))]
mod guard;
#[cfg_attr(not(windows), allow(dead_code))]
mod input;
#[cfg_attr(not(windows), allow(dead_code))]
mod keys;
#[cfg_attr(not(windows), allow(dead_code))]
mod outbox;
#[cfg_attr(not(windows), allow(dead_code))]
mod output;
#[cfg_attr(not(windows), allow(dead_code))]
mod queries;
#[cfg_attr(not(windows), allow(dead_code))]
mod reply;
#[cfg_attr(not(windows), allow(dead_code))]
mod screen;
#[cfg_attr(not(windows), allow(dead_code))]
mod words;

#[cfg(windows)]
mod client;
#[cfg(windows)]
mod server;
#[cfg(windows)]
mod sys;

#[cfg(windows)]
pub use client::{Attached, Connection, Endpoint, connect, connect_or_start};
#[cfg(windows)]
pub use server::main as serve;
