//! State and rules for the Files page.
pub mod diff;
pub mod format;
pub mod kinds;
pub mod listing;
pub mod pane;
pub mod paths;
pub mod transfers;
pub mod zoom;

use serde::{Deserialize, Serialize};

/// A file or folder pinned on one node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub node: String,
    pub path: String,
}

/// What Files keeps between app runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    pub panes: [pane::Kept; 2],
    pub transfers: Vec<transfers::Kept>,
    pub sidecar_open: bool,
    pub sidecar: pane::Kept,
    pub pins: Vec<Pin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom: zoom::Level,
}
