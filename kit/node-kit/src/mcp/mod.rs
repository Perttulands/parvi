//! MCP: the areas' commands as tools for agents and AI apps. Every node
//! serves them over streamable HTTP at `/mcp`, and `parvi mcp` offers the
//! same over stdio by relaying to the node. Every call is logged. There are
//! no permissions here: a caller reaches what the node's user does.

mod call_log;
mod http;
mod rpc;
mod stdio;

use axum::Router;
use parvi_core::{Error, NodeId};
use serde_json::{Value, json};

use crate::Command;

pub use http::PATH;
pub use stdio::stdio;

/// The tools: every built area's commands.
pub(crate) struct Mcp {
    node: NodeId,
    commands: Vec<Command>,
}

impl Mcp {
    pub(crate) fn new(node: NodeId, commands: Vec<Command>) -> Mcp {
        Mcp { node, commands }
    }

    pub(crate) fn routes(self) -> Router {
        http::routes(std::sync::Arc::new(self))
    }

    /// Every tool, as `tools/list` answers. Each takes `node`, the node it
    /// runs on.
    fn list(&self) -> Value {
        let tools: Vec<Value> = self
            .commands
            .iter()
            .map(|command| {
                let mut schema = (command.schema)();
                schema["properties"]["node"] = json!({
                    "type": "string",
                    "description": "The node to run it on: its id; this node if absent.",
                });
                json!({
                    "name": command.name,
                    "description": command.description,
                    "inputSchema": schema,
                })
            })
            .collect();
        json!({ "tools": tools })
    }

    fn command(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|command| command.name == name)
    }

    /// Takes `node` out of a call's arguments: this node unless they name
    /// another, which this node cannot reach.
    fn target(&self, args: &mut Value) -> Result<(), Error> {
        let named = args.as_object_mut().and_then(|args| args.remove("node"));
        match named {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(wanted)) if wanted == self.node.as_str() => Ok(()),
            Some(Value::String(wanted)) => Err(Error::new(
                "unknown_node",
                format!("node {wanted} is not this node, and this node reaches no other"),
            )),
            Some(_) => Err(Error::new("invalid_argument", "node is a string")),
        }
    }
}
