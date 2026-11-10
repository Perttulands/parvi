//! Wire types for the Files area.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Entry {
    pub name: String,
    pub folder: bool,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub modified: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Listing {
    /// The folder's own path, links resolved.
    pub path: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Root {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Match {
    pub path: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Matches {
    pub matches: Vec<Match>,
    pub truncated: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Diff {
    pub path: String,
    pub repository: String,
    pub diff: String,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
