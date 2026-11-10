//! What a node area plugs into (ARCHITECTURE.md, Areas register themselves):
//! its routes, the frames it owns on the socket, and its commands, each of
//! which is both an MCP tool and a `parvi <noun> <verb>`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::Router;
use parvi_core::Error;
use parvi_core::frame::Kinds;
use serde_json::Value;

use crate::{Provided, Setup};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Builds an area from the node's setup, once, when the node starts. The
/// binary holds a list of these, by name; an area whose config section says
/// `enabled = false` is not built.
pub type Build = fn(&Setup) -> anyhow::Result<Built>;

/// What a [`Build`] answers. Areas are built in two steps, so their order in
/// the binary's list never matters: each is built and fills the slots it
/// provides ([`Setup::provided`]); then those that use what others provide
/// finish, reading those slots.
pub enum Built {
    /// The area, which uses nothing another area provides.
    Now(Arc<dyn Area>),
    /// How the area finishes once every area is built.
    Then(Finish),
}

/// The second step of building an area: given what areas provide.
pub type Finish = Box<dyn FnOnce(&Provided) -> anyhow::Result<Arc<dyn Area>> + Send>;

impl Built {
    pub fn now(area: impl Area) -> anyhow::Result<Built> {
        Ok(Built::Now(Arc::new(area)))
    }

    pub fn then(
        finish: impl FnOnce(&Provided) -> anyhow::Result<Arc<dyn Area>> + Send + 'static,
    ) -> anyhow::Result<Built> {
        Ok(Built::Then(Box::new(finish)))
    }
}

pub trait Area: Send + Sync + 'static {
    /// Its config section, its name in the hello's `off`, and in errors.
    fn name(&self) -> &'static str;

    /// The frame kinds it owns, both ways. A client's frame of one of these
    /// goes to its channel.
    fn kinds(&self) -> Kinds {
        &[]
    }

    fn routes(&self) -> Router {
        Router::new()
    }

    fn commands(&self) -> Vec<Command> {
        Vec::new()
    }

    /// Its side of one client's socket; `None` for an area with no frames.
    fn channel(&self, _client: Client) -> Option<Box<dyn Channel>> {
        None
    }

    /// Stops what it runs, as the node stops.
    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// One client's socket, as an area sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Client {
    /// Unique among this node's connections.
    pub id: u64,
}

/// An area's side of one socket. Dropped when the client goes.
pub trait Channel: Send {
    /// A frame from the client, of one of the area's kinds.
    fn receive(&mut self, frame: Vec<u8>) -> BoxFuture<'_, ()>;

    /// The next frame to push to the client: pending while there is none,
    /// `None` once the channel has ended, which closes the socket. It must
    /// be cancel-safe: the node drops it whenever another channel or the
    /// client is first.
    fn next(&mut self) -> BoxFuture<'_, Option<Vec<u8>>>;
}

type Call = dyn Fn(Value) -> BoxFuture<'static, Result<Value, Error>> + Send + Sync;

/// One definition behind an MCP tool `noun_verb` and the CLI's
/// `parvi noun verb`.
#[derive(Clone)]
pub struct Command {
    /// `noun_verb`: one underscore, words on both sides of it.
    pub name: &'static str,
    pub description: &'static str,
    /// The arguments' JSON Schema, asked for each time the tools are listed,
    /// so it may offer what the node has now.
    pub schema: Arc<dyn Fn() -> Value + Send + Sync>,
    pub call: Arc<Call>,
}

impl Command {
    pub fn new<F, Fut>(
        name: &'static str,
        description: &'static str,
        schema: impl Fn() -> Value + Send + Sync + 'static,
        call: F,
    ) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, Error>> + Send + 'static,
    {
        Self {
            name,
            description,
            schema: Arc::new(schema),
            call: Arc::new(move |args| Box::pin(call(args))),
        }
    }

    /// Whether the name reads as `noun_verb`.
    pub fn well_named(&self) -> bool {
        let mut parts = self.name.split('_');
        matches!(
            (parts.next(), parts.next(), parts.next()),
            (Some(noun), Some(verb), None) if !noun.is_empty() && !verb.is_empty()
        )
    }
}
