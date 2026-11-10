//! Reads `bd --json list --status all`'s answer into [`Item`]s, with each
//! Bead's family worked out from the whole store.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use parvi_work_contract::WorkRef;
use serde::Deserialize;

use super::item::{Family, Item};

/// Every Bead in the store at `folder` on `node`, newest update first.
pub fn parse(json: &str, node: &str, folder: &str) -> Result<Vec<Item>> {
    let raw: Vec<RawBead> = serde_json::from_str(json).context("bd's answer is not a Bead list")?;
    let ids: HashSet<&str> = raw.iter().map(|b| b.id.as_str()).collect();
    let reference = |id: &str| -> String {
        if ids.contains(id) {
            WorkRef::Bead {
                node: node.into(),
                folder: folder.into(),
                id: id.into(),
            }
            .to_string()
        } else {
            id.to_owned()
        }
    };
    let mut children: HashMap<&str, Vec<String>> = HashMap::new();
    let mut blocking: HashMap<&str, Vec<String>> = HashMap::new();
    for bead in &raw {
        if let Some(parent) = &bead.parent {
            children
                .entry(parent)
                .or_default()
                .push(reference(&bead.id));
        }
        for dep in bead.dependencies.iter().filter(|d| d.kind == "blocks") {
            blocking
                .entry(&dep.depends_on_id)
                .or_default()
                .push(reference(&bead.id));
        }
    }
    let mut items: Vec<Item> = raw
        .iter()
        .map(|bead| Item {
            reference: reference(&bead.id),
            id: bead.id.clone(),
            title: bead.title.clone(),
            status: bead.status.clone(),
            kind: bead.issue_type.clone(),
            priority: bead.priority,
            url: None,
            author: bead.created_by.clone(),
            assignees: bead.assignee.iter().cloned().collect(),
            labels: bead.labels.clone(),
            created: bead.created_at.clone(),
            updated: bead.updated_at.clone(),
            closed: bead.closed_at.clone(),
            body: body(bead),
            family: Family {
                parent: bead.parent.as_deref().map(reference),
                children: children.remove(bead.id.as_str()).unwrap_or_default(),
                blockers: bead
                    .dependencies
                    .iter()
                    .filter(|d| d.kind == "blocks")
                    .map(|d| reference(&d.depends_on_id))
                    .collect(),
                blocking: blocking.remove(bead.id.as_str()).unwrap_or_default(),
                pulls: Vec::new(),
            },
        })
        .collect();
    items.sort_by(|a, b| b.updated.cmp(&a.updated));
    Ok(items)
}

/// The description, then the other written fields under headings.
fn body(bead: &RawBead) -> String {
    let mut body = bead.description.trim().to_owned();
    for (heading, text) in [
        ("Design", &bead.design),
        ("Acceptance criteria", &bead.acceptance_criteria),
        ("Notes", &bead.notes),
        ("Close reason", &bead.close_reason),
    ] {
        if let Some(text) = text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            body.push_str(&format!("## {heading}\n\n{text}"));
        }
    }
    body
}

#[derive(Deserialize)]
struct RawBead {
    id: String,
    title: String,
    status: String,
    #[serde(default)]
    priority: Option<u8>,
    #[serde(default)]
    issue_type: Option<String>,
    #[serde(default)]
    created_by: Option<String>,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    closed_at: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    design: Option<String>,
    #[serde(default)]
    acceptance_criteria: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    close_reason: Option<String>,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    dependencies: Vec<RawDependency>,
}

#[derive(Deserialize)]
struct RawDependency {
    depends_on_id: String,
    #[serde(rename = "type")]
    kind: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/work/beads.json");

    fn at(id: &str) -> String {
        format!("bead:n1:/srv/tst#{id}")
    }

    #[test]
    fn reads_every_bead_with_its_family() {
        let items = parse(FIXTURE, "n1", "/srv/tst").unwrap();
        let ids: Vec<_> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids.len(), 4);
        let find = |id: &str| items.iter().find(|i| i.id == id).unwrap();
        let epic = find("tst-qq9");
        assert_eq!(epic.reference, at("tst-qq9"));
        assert_eq!(epic.kind.as_deref(), Some("epic"));
        assert_eq!(epic.family.children, vec![at("tst-qq9.1"), at("tst-qq9.2")]);
        let reading = find("tst-qq9.1");
        assert_eq!(reading.status, "in_progress");
        assert_eq!(reading.assignees, vec!["agent-one"]);
        assert_eq!(reading.labels, vec!["beads", "work"]);
        assert_eq!(
            reading.family.parent.as_deref(),
            Some(at("tst-qq9").as_str())
        );
        assert_eq!(reading.family.blocking, vec![at("tst-qq9.2")]);
        let failed = find("tst-qq9.2");
        // A blocker in another store stays a bare id, placed by its prefix.
        assert_eq!(
            failed.family.blockers,
            vec![at("tst-qq9.1"), "ctx-5t7".into()]
        );
        let old = find("tst-1mt");
        assert_eq!(old.status, "closed");
        assert!(old.closed.is_some());
        assert!(
            old.body
                .ends_with("## Close reason\n\nSuperseded by the seam")
        );
        assert!(
            items.windows(2).all(|w| w[0].updated >= w[1].updated),
            "newest first"
        );
    }

    #[test]
    fn a_wrong_answer_is_an_error_not_an_empty_store() {
        assert!(parse("{\"error\":\"x\"}", "n", "/x").is_err());
        assert!(parse("", "n", "/x").is_err());
    }
}
