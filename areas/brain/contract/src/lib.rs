use serde::{Deserialize, Serialize};

/// Stable error codes returned by Brain.
pub const BAD_REQUEST: &str = "bad_request";
pub const FORBIDDEN: &str = "forbidden";
pub const NOT_FOUND: &str = "not_found";
pub const CONFLICT: &str = "conflict";
pub const TOO_LARGE: &str = "too_large";
pub const UNAVAILABLE: &str = "unavailable";
pub const BRAIN_ERROR: &str = "brain_error";

/// One hit, in Citadel's order: best first. The snippet marks matched words
/// between `»` and `«`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Found {
    pub path: String,
    pub title: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub collection: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub snippet: String,
}

/// One page as the reader shows it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Page {
    pub path: String,
    pub title: String,
    pub kind: String,
    pub summary: String,
    /// The front matter as written, delimiters included; with the body it
    /// is the whole file, which is what an edit starts from.
    pub front: String,
    /// The text after the front matter.
    pub body: String,
    /// Each link target the page writes, once, in order.
    pub links: Vec<Link>,
    /// The pages linking here, by title.
    pub backlinks: Vec<PageRef>,
    /// When the page last changed and who changed it, from its history.
    pub updated: String,
    pub author: String,
    /// The commits that touched it, newest first.
    pub history: Vec<Commit>,
    /// Why there is no history, when git would not read the corpus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_error: Option<String>,
}

/// A top-level folder of the corpus and how many pages it holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Collection {
    pub name: String,
    pub pages: usize,
}

/// A link target as written, and the page it names when there is one.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Link {
    pub target: String,
    pub page: Option<PageRef>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PageRef {
    pub path: String,
    pub title: String,
    pub kind: String,
}

/// One commit of a page's history.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Commit {
    pub hash: String,
    /// When it was committed, ISO 8601.
    pub time: String,
    pub author: String,
    pub message: String,
}

/// A commit and the pages it touched that are still there.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Change {
    #[serde(flatten)]
    pub commit: Commit,
    #[serde(default)]
    pub pages: Vec<PageRef>,
}

/// A collection's page and its last change.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Listed {
    #[serde(flatten)]
    pub page: PageRef,
    pub updated: String,
    pub author: String,
}

impl Page {
    pub fn text(&self) -> String {
        format!("{}{}", self.front, self.body)
    }
}

/// Search results in Citadel's ranking order.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Results {
    pub results: Vec<Found>,
}

/// The corpus's collections.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Collections {
    pub collections: Vec<Collection>,
}

/// A collection's pages and any history error.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Listing {
    pub pages: Vec<Listed>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Recent changes and any history error.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Changes {
    pub changes: Vec<Change>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Body of a page save.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Save {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub summary: String,
}
