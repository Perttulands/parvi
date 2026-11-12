//! State and rules for the Files page.
pub mod diff;
pub mod drafts;
pub mod format;
pub mod kinds;
pub mod listing;
pub mod pane;
pub mod paths;
pub mod zoom;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub node: String,
    pub path: String,
    pub folder: bool,
}

/// What Files keeps between app runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    pub pane: pane::Kept,
    pub sidecar_open: bool,
    pub sidecar: pane::Kept,
    pub dock: pane::Kept,
    pub dock_file: Option<String>,
    pub pins: Vec<Pin>,
    pub download_folder: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom: zoom::Level,
}
