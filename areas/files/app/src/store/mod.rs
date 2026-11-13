//! State and rules for the Files page.
pub mod diff;
pub mod drafts;
pub mod find;
pub mod format;
pub mod kinds;
pub mod listing;
pub mod paths;
pub mod tree;
pub mod zoom;

use serde::{Deserialize, Serialize};

/// What Files keeps between app runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    pub tree: tree::Kept,
    /// The file the page showed last.
    pub file: Option<String>,
    pub sidecar_open: bool,
    pub sidecar: tree::Kept,
    pub dock: tree::Kept,
    pub dock_file: Option<String>,
    pub download_folder: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom: zoom::Level,
}
