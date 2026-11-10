//! What the Work tab shows of each project, sent to browsers as JSON: a
//! GitHub repository's issues and pull requests, shaped from GitHub's answer
//! by [`super::parse`], or a Beads store's Beads as [`Item`]s. Times are the
//! trackers' ISO 8601 strings; browsers format them.

use serde::{Deserialize, Serialize};

use super::item::Item;

/// Every configured project, in configuration order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub projects: Vec<ProjectView>,
}

/// One project as last read, tagged by its tracker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tracker", rename_all = "lowercase")]
pub enum ProjectView {
    Github(RepoView),
    Beads(StoreView),
}

/// One repository as last read, and how that went.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoView {
    /// `owner/name` as configured.
    pub name: String,
    /// Where a session for one of its issues starts on this node, if anywhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
    /// When it was last read, in Unix ms; absent until the first read worked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synced: Option<u64>,
    /// Why the last read failed. The issues and pull requests are then the
    /// last ones read, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Absent until the first read worked.
    #[serde(flatten)]
    pub work: Option<Work>,
}

/// One Beads store as last read, and how that went. A store that failed
/// says so; its Beads are then the last ones read, if any, never an empty
/// list standing in for a failure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreView {
    /// The project folder whose `.beads` holds the store.
    pub folder: String,
    /// When it was last read, in Unix ms; absent until the first read worked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synced: Option<u64>,
    /// Why the last read failed. A failed store is tried again when it
    /// changes or watching starts anew.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Present when Parvi cannot see this store change, saying why: its
    /// Beads are then as read when watching began.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unwatched: Option<String>,
    /// Every Bead, newest update first; absent until the first read worked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<Item>>,
}

/// What GitHub says about a repository's issues and pull requests.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Work {
    pub url: String,
    /// GitHub's totals, which may exceed the items read.
    pub open_issues: u32,
    pub closed_issues: u32,
    pub open_pulls: u32,
    pub closed_pulls: u32,
    /// Open first, then recently closed; each newest update first.
    pub issues: Vec<Issue>,
    pub pulls: Vec<Pull>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueState {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: u32,
    pub title: String,
    pub url: String,
    pub state: IssueState,
    /// Absent for a deleted account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub created: String,
    pub updated: String,
    pub body: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    /// Branches created for it from GitHub's "Development" box.
    pub branches: Vec<String>,
    /// Pull requests that close it when merged.
    pub pulls: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullState {
    Open,
    Draft,
    Merged,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pull {
    pub number: u32,
    pub title: String,
    pub url: String,
    pub state: PullState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub created: String,
    pub updated: String,
    pub body: String,
    /// The branch it merges, and the branch it merges into.
    pub head: String,
    pub base: String,
    pub additions: u32,
    pub deletions: u32,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    /// Issues it closes when merged.
    pub closes: Vec<u32>,
    /// The latest commit's checks; absent when it has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checks: Option<Checks>,
}

/// The latest commit's check runs and commit statuses, counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checks {
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn snapshot_round_trips_before_and_after_a_read() {
        for work in [None, Some(Work::default())] {
            let snapshot = Snapshot {
                projects: vec![ProjectView::Github(RepoView {
                    name: "example/project".into(),
                    work,
                    ..Default::default()
                })],
            };
            let wire = serde_json::to_vec(&snapshot).unwrap();
            assert_eq!(serde_json::from_slice::<Snapshot>(&wire).unwrap(), snapshot);
        }
    }
}
