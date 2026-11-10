//! Wire types for the Files area.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub folder: bool,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub modified: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Listing {
    /// The folder's own path, links resolved.
    pub path: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Root {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Match {
    pub path: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Matches {
    pub matches: Vec<Match>,
    pub truncated: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Diff {
    pub path: String,
    pub repository: String,
    pub diff: String,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Roots exposed by a node.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct NodeRoots {
    pub node: String,
    pub roots: Vec<Root>,
}

/// A copy started on this node.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TransferRequest {
    pub node: String,
    pub path: String,
    pub target_node: String,
    pub folder: String,
    pub id: String,
}

/// Progress of a copy started on this node.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Progress {
    pub state: String,
    pub bytes: u64,
    pub total: u64,
    pub error: Option<String>,
}

/// One file or folder on the addressed node.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct At {
    pub node: String,
    pub path: String,
}

/// A save may name the version it was edited from.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SaveAt {
    pub node: String,
    pub path: String,
    pub base: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Search {
    pub node: String,
    pub path: String,
    pub q: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Move {
    pub node: String,
    pub path: String,
    pub to: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Job {
    pub node: String,
    pub id: String,
}

/// An attachment written for a session.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DropAt {
    pub node: Option<String>,
    pub name: String,
    #[serde(default)]
    pub user: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Dropped {
    pub path: std::path::PathBuf,
}

pub const OUTSIDE_ROOTS: &str = "outside_roots";
pub const ROOT_IMMUTABLE: &str = "root_immutable";
pub const NOT_FOUND: &str = "not_found";
pub const ALREADY_EXISTS: &str = "already_exists";
pub const FILE_CHANGED: &str = "file_changed";
pub const PERMISSION_DENIED: &str = "permission_denied";
pub const NOT_FOLDER: &str = "not_folder";
pub const NOT_FILE: &str = "not_file";
pub const TOO_LARGE: &str = "too_large";
pub const UNKNOWN_NODE: &str = "unknown_node";
pub const IO_ERROR: &str = "io_error";
