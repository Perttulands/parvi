//! Every resource is a node and an id local to it, so a fleet of nodes is
//! one namespace without a central registry.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A node's id: stable across restarts and upgrades.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for NodeId {
    fn from(id: &str) -> Self {
        NodeId(id.to_owned())
    }
}

/// A resource: the node that has it and its id there.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct Ref<Id = String> {
    pub node: NodeId,
    pub id: Id,
}

impl<Id> Ref<Id> {
    pub fn new(node: NodeId, id: Id) -> Self {
        Ref { node, id }
    }
}
