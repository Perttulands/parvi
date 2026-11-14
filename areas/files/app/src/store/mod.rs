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
    /// The panel beside the terminals is open, and what it shows: its
    /// tree, and the file open in it.
    pub beside_open: bool,
    pub beside: tree::Kept,
    pub beside_file: Option<String>,
    pub download_folder: String,
    /// Long lines run past the edge instead of wrapping.
    pub unwrapped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom: zoom::Level,
}
