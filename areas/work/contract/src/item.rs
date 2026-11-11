//! One work item in the shape every tracker shares: what Missions, Send,
//! MCP and terminal links read through [`super::Work::item`], and what a
//! Beads store's view lists.

use serde::{Deserialize, Serialize};

/// A Bead or a GitHub issue.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// The canonical reference (`docs/work.md`, References).
    pub reference: String,
    /// The id within its tracker: `ctx-12`, or the issue's number.
    pub id: String,
    pub title: String,
    /// The tracker's own status: Beads' `open`, `in_progress`, `blocked`,
    /// `deferred`, `closed` and any custom one; GitHub's `open` or `closed`.
    pub status: String,
    /// Beads' issue type, such as `task` or `epic`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Beads' priority, 0 the highest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub assignees: Vec<String>,
    pub labels: Vec<String>,
    pub created: String,
    pub updated: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed: Option<String>,
    /// Markdown: the description, then a Bead's design, acceptance criteria,
    /// notes and close reason under their own headings.
    pub body: String,
    pub family: Family,
}

/// The items an item is linked with. Each is a reference: canonical within
/// the item's own store or for GitHub, and a bare Bead id for a Bead in
/// another store, which [`super::Work::resolve`] places by its prefix.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Family {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub children: Vec<String>,
    /// What must finish before this can.
    pub blockers: Vec<String>,
    /// What waits on this.
    pub blocking: Vec<String>,
    /// Pull requests that close this issue when merged.
    pub pulls: Vec<String>,
}

impl Item {
    /// The one line that names the item to an agent, as CHROTE's Send
    /// writes it: `bead <id>: <title>`, with the store's folder after the id
    /// when `store` is given because the id alone does not place it, or
    /// `issue <owner>/<repo>#<n>: <title>`.
    pub fn reference_line(&self, store: Option<&str>) -> String {
        let name = match parvi_work_ref::parse_resolved(&self.reference) {
            Ok(r @ parvi_work_ref::WorkRef::Issue { .. }) => format!("issue {}", r.short()),
            _ => match store {
                Some(folder) => format!("bead {} ({folder})", self.id),
                None => format!("bead {}", self.id),
            },
        };
        match self.title.trim() {
            "" => name,
            title => format!("{name}: {title}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_an_item_as_chrote_sends_it() {
        let bead = Item {
            reference: "bead:n:/srv/x#x-1".into(),
            id: "x-1".into(),
            title: " Read stores ".into(),
            ..Item::default()
        };
        assert_eq!(bead.reference_line(None), "bead x-1: Read stores");
        assert_eq!(
            bead.reference_line(Some("/srv/x")),
            "bead x-1 (/srv/x): Read stores"
        );
        let issue = Item {
            reference: "gh:a/b#3".into(),
            id: "3".into(),
            ..Item::default()
        };
        assert_eq!(issue.reference_line(None), "issue a/b#3");
    }
}
