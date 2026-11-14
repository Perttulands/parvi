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

/// A file and when it last changed, in milliseconds since the Unix epoch;
/// or, with `files`, a folder in which that many files changed together
/// (a checkout, a batch of renders), `modified` the newest of them.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Changed {
    pub path: String,
    pub modified: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<usize>,
}

/// The files changed most recently under the roots, newest first.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Recent {
    pub files: Vec<Changed>,
}

/// When each of some paths last changed: what a view that shows them asks
/// to learn whether to read them again. A path that is gone (or outside
/// the roots) has none.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Stat {
    pub paths: Vec<String>,
}

/// A copy started on this node.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TransferRequest {
    #[serde(default)]
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
    #[serde(default)]
    pub node: String,
    pub path: String,
}

/// A save may name the version it was edited from.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SaveAt {
    #[serde(default)]
    pub node: String,
    pub path: String,
    pub base: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Search {
    #[serde(default)]
    pub node: String,
    /// The folder searched; a name find without one looks under every root.
    #[serde(default)]
    pub path: String,
    /// For a find under every root, the folder the asker stands in: its
    /// files come first, and it is covered even where the roots' walk
    /// leaves it out.
    #[serde(default)]
    pub near: String,
    /// Answer only the files under `near`, at once.
    #[serde(default)]
    pub beside: bool,
    pub q: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Move {
    #[serde(default)]
    pub node: String,
    pub path: String,
    pub to: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Job {
    #[serde(default)]
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
