//! Role cards: role text a slot may carry, and nothing it runs.
//!
//! Every node has seven built-in generic roles. A card in the roles
//! directory, `<id>.toml`, adds a role or overrides a built-in one. A card
//! that cannot be read is skipped and named, so one bad card never hides the
//! rest.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::DefinitionError;
use crate::links;

pub const ROLE_SCHEMA: i64 = 1;
pub const RETIRED: &str = "retired";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleFile {
    schema: i64,
    card: RoleCardFile,
    #[serde(default, rename = "note")]
    notes: Vec<RoleNote>,
}

// Hand-authored role cards may carry extra text fields. They are retained by
// the editor, while harness settings are refused because roles are text only.
#[derive(Default, Deserialize)]
#[serde(default)]
struct RoleCardFile {
    id: String,
    display_name: String,
    kind: String,
    summary: String,
    tags: Vec<String>,
    status: String,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RoleCard {
    pub id: String,
    pub display_name: String,
    pub kind: String,
    pub summary: String,
    pub tags: Vec<String>,
    /// `active`, or `retired`: no longer offered, and a slot naming it
    /// stops validation until it is restaffed or the role comes back.
    pub status: String,
    #[serde(skip)]
    pub notes: Vec<RoleNote>,
    /// Whether this is one of the built-in roles, customized or not.
    #[serde(skip)]
    pub builtin: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RoleNote {
    pub ts: String,
    pub actor: String,
    pub text: String,
}

/// The generic roles every node has: id, display name and kind, summary,
/// capabilities.
const BUILTIN: &[(&str, &str, &str, &[&str])] = &[
    (
        "scout",
        "Scout",
        "Explores a codebase read-only before changes begin: maps files, callers, tests, risks and unknowns, and reports the evidence.",
        &["research", "inspect", "read-only"],
    ),
    (
        "planner",
        "Planner",
        "Turns grounded requirements into an ordered implementation plan with explicit verification.",
        &["planning", "design"],
    ),
    (
        "builder",
        "Builder",
        "Implements scoped changes, exercises them, and leaves a working artifact.",
        &["implement", "test"],
    ),
    (
        "judge",
        "Judge",
        "Judges work against its acceptance criteria, scope, safety and evidence, and returns a clear pass or fail verdict with the blocking findings.",
        &["judge", "verify", "review"],
    ),
    (
        "orchestrator",
        "Orchestrator",
        "Coordinates role-specialized agents, dependencies, handoffs, and completion gates.",
        &["orchestrate", "coordinate"],
    ),
    (
        "debugger",
        "Debugger",
        "Diagnoses failures from evidence, isolates root causes, and verifies repairs.",
        &["debug", "diagnose"],
    ),
    (
        "reviewer",
        "Reviewer",
        "Reviews changes independently for correctness, regressions, and maintainability.",
        &["review", "audit"],
    ),
];

pub(crate) fn builtin(id: &str) -> Option<RoleCard> {
    let (id, name, summary, tags) = BUILTIN.iter().find(|(builtin, ..)| *builtin == id)?;
    Some(RoleCard {
        id: (*id).to_owned(),
        display_name: (*name).to_owned(),
        // A built-in role's kind is its id.
        kind: (*id).to_owned(),
        summary: (*summary).to_owned(),
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        status: "active".to_owned(),
        notes: Vec::new(),
        builtin: true,
    })
}

/// A role card a listing skipped, and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Unreadable {
    pub name: String,
    pub reason: String,
}

/// The roles directory of a state directory.
#[derive(Clone, Debug)]
pub struct RoleStore {
    dir: PathBuf,
}

impl RoleStore {
    pub fn new(dir: impl Into<PathBuf>) -> RoleStore {
        RoleStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads one role: its card, else the built-in role of that id.
    pub fn read(&self, id: &str) -> Result<RoleCard, DefinitionError> {
        check_role_id(id)?;
        let text = match links::read_definition(&self.dir.join(format!("{id}.toml"))) {
            Err(DefinitionError::NotFound(_)) => {
                return builtin(id)
                    .ok_or_else(|| DefinitionError::NotFound(format!("role {id:?}")));
            }
            other => other?,
        };
        let mut card = parse_card(id, &text)?;
        card.builtin = builtin(id).is_some();
        Ok(card)
    }

    /// Every role, built-ins included, by id, and the cards it could not read.
    pub fn list(&self) -> Result<(Vec<RoleCard>, Vec<Unreadable>), DefinitionError> {
        let mut cards: Vec<RoleCard> = BUILTIN.iter().filter_map(|(id, ..)| builtin(id)).collect();
        let mut unreadable = Vec::new();
        for name in links::list_names(&self.dir, ".toml")? {
            match self.read(&name) {
                Ok(card) => {
                    cards.retain(|existing| existing.id != card.id);
                    cards.push(card);
                }
                Err(err) => unreadable.push(Unreadable {
                    name,
                    reason: err.to_string(),
                }),
            }
        }
        cards.sort_by(|a, b| a.id.cmp(&b.id));
        Ok((cards, unreadable))
    }
}

pub(crate) fn parse_card(id: &str, text: &str) -> Result<RoleCard, DefinitionError> {
    let file: RoleFile = toml::from_str(text)
        .map_err(|err| DefinitionError::InvalidSource(format!("role card {id:?}: {err}")))?;
    if file.schema != ROLE_SCHEMA {
        return Err(DefinitionError::UnsupportedSchema(format!(
            "role card {id:?} has schema {}",
            file.schema
        )));
    }
    let mut missing = Vec::new();
    if file.card.id.is_empty() {
        missing.push("card.id");
    }
    if file.card.kind.is_empty() {
        missing.push("card.kind");
    }
    if !missing.is_empty() {
        return Err(DefinitionError::InvalidSource(format!(
            "role card {id:?} is missing {}",
            missing.join(", ")
        )));
    }
    if file.card.id != id {
        return Err(DefinitionError::InvalidSource(format!(
            "role card file {id:?} holds card.id {:?}; they must match",
            file.card.id
        )));
    }
    for field in ["harness", "model", "effort"] {
        if file.card.extra.contains_key(field) {
            return Err(DefinitionError::InvalidSource(format!(
                "role card {id:?} cannot set {field}"
            )));
        }
    }
    Ok(RoleCard {
        id: file.card.id,
        display_name: file.card.display_name,
        kind: file.card.kind,
        summary: file.card.summary,
        tags: file.card.tags,
        status: file.card.status,
        notes: file.notes,
        builtin: false,
    })
}

/// A role id is lowercase letters, digits and inner dashes.
pub(crate) fn check_role_id(id: &str) -> Result<(), DefinitionError> {
    let valid = !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(DefinitionError::InvalidName(format!(
            "role {id:?} must be lowercase letters, digits and dashes"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("parvi-64-roles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn built_in_roles_answer_without_a_directory() {
        let store = RoleStore::new("/nonexistent/parvi-64");
        let judge = store.read("judge").unwrap();
        assert!(judge.builtin);
        assert_eq!(judge.kind, "judge");
        assert_eq!(store.read("critic").unwrap_err().code(), "not_found");
        assert_eq!(store.read("Critic").unwrap_err().code(), "invalid_name");
        assert_eq!(store.list().unwrap().0.len(), 7);
    }

    #[test]
    fn a_card_adds_or_overrides_a_role_and_a_bad_one_is_named() {
        let dir = scratch("cards");
        std::fs::write(
            dir.join("judge.toml"),
            "schema = 1\n[card]\nid = \"judge\"\nkind = \"judge\"\nstatus = \"retired\"\n\n[[note]]\nts = \"2026-11-05T00:00:00Z\"\nactor = \"human:ui\"\ntext = \"Retired for now\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("critic.toml"),
            "schema = 1\n[card]\nid = \"critic\"\nkind = \"reviewer\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("broken.toml"),
            "schema = 1\n[card]\nid = \"broken\"\nharness = \"pi\"\n",
        )
        .unwrap();
        let store = RoleStore::new(&dir);
        let judge = store.read("judge").unwrap();
        assert_eq!(
            (judge.status.as_str(), judge.builtin, judge.notes.len()),
            (RETIRED, true, 1)
        );
        let (cards, unreadable) = store.list().unwrap();
        assert_eq!(cards.len(), 8);
        assert_eq!(unreadable.len(), 1);
        assert_eq!(unreadable[0].name, "broken");
        assert!(
            unreadable[0].reason.contains("invalid_definition_source"),
            "{}",
            unreadable[0].reason
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// A slot that refers to a role.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleUse {
    pub mission_id: String,
    pub mission_slug: String,
    pub mission_title: String,
    pub formation_id: String,
    pub formation_title: String,
    pub slot_id: String,
    pub slot_label: String,
}

/// Known uses and any sources that prevented a complete scan.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RoleUsage {
    pub usage: Vec<RoleUse>,
    pub complete: bool,
    pub unreadable: Vec<Unreadable>,
}

impl RoleStore {
    /// Scans every mission, retaining known uses even if one is unreadable.
    pub fn usage(&self, id: &str) -> Result<RoleUsage, crate::AuthoringError> {
        let root = self.dir.parent().unwrap_or(&self.dir);
        let locks = crate::write::locks(root, "role-references");
        let _guard = locks.references.write().unwrap_or_else(|e| e.into_inner());
        self.usage_locked(id)
    }

    pub(crate) fn usage_locked(&self, id: &str) -> Result<RoleUsage, crate::AuthoringError> {
        check_role_id(id)?;
        let root = self.dir.parent().ok_or_else(|| {
            crate::AuthoringError::new("INVALID_AGENT_CARD", "role directory has no mission root")
        })?;
        let store = crate::Store::new(root);
        let mut report = RoleUsage {
            usage: Vec::new(),
            complete: true,
            unreadable: Vec::new(),
        };
        for listing in store.list_missions()? {
            let mission = match store.read_mission(&listing.slug) {
                Ok(mission) => mission,
                Err(error) => {
                    report.unreadable.push(Unreadable {
                        name: listing.slug,
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            for formation in &mission.formations {
                for slot in &formation.slots {
                    if slot.agent_id == id {
                        report.usage.push(RoleUse {
                            mission_id: mission.id.clone(),
                            mission_slug: listing.slug.clone(),
                            mission_title: mission.title.clone(),
                            formation_id: formation.id.clone(),
                            formation_title: formation.title.clone(),
                            slot_id: slot.id.clone(),
                            slot_label: slot.label.clone(),
                        });
                    }
                }
            }
        }
        report.complete = report.unreadable.is_empty();
        Ok(report)
    }
}
