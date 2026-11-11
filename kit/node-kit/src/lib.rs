//! What every Parvi node is, whatever areas it serves (ARCHITECTURE.md,
//! Node kit): who it is, its config, its server with the trust check, the
//! socket's hello and frame routing, MCP and the CLI over the same commands,
//! and the slots areas fill for one another.

pub mod address;
mod area;
pub mod args;
pub mod cli;
mod client;
pub mod config;
pub mod events;
pub mod hook;
pub mod identity;
pub mod listing;
pub mod mcp;
pub mod own;
mod ring;
pub mod serve;
mod setup;
mod view;

pub use area::{Area, BoxFuture, Build, Built, Channel, Client, Command, Finish};
pub use client::{NodeClient, NodeError};
pub use config::Config;
pub use listing::Listing;
pub use serve::{Failure, Options, Running, start};
pub use setup::{Provided, Setup};

/// This build: the package version and, built from Git, its commit
/// (`0.0.1+8cdb58b`). `/api/health`, the hello and `parvi --version` say it.
pub const VERSION: &str = env!("PARVI_VERSION");
