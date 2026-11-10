//! Role mutations share the mission write lock with usage checks.
use crate::{AuthoringError, RoleCard, RoleStore};
use toml_edit::{Array, DocumentMut, value};

#[derive(Clone, Debug)]
pub struct RoleEdit {
    pub card: RoleCard,
    pub etag: String,
}

fn new_document(card: &RoleCard) -> DocumentMut {
    let mut doc = DocumentMut::new();
    doc["schema"] = value(1);
    doc["card"] = toml_edit::Item::Table(toml_edit::Table::new());
    doc["card"]["id"] = value(card.id.clone());
    doc["card"]["display_name"] = value(card.display_name.clone());
    doc["card"]["kind"] = value(card.kind.clone());
    doc["card"]["summary"] = value(card.summary.clone());
    let mut tags = Array::new();
    for tag in &card.tags {
        tags.push(tag.as_str());
    }
    doc["card"]["tags"] = value(tags);
    doc["card"]["status"] = value(card.status.clone());
    doc
}

impl RoleStore {
    pub(crate) fn source(&self, id: &str) -> Result<String, AuthoringError> {
        crate::roles::check_role_id(id)
            .map_err(|e| AuthoringError::new("INVALID_AGENT_CARD", e.to_string()))?;
        match crate::links::read_definition(&self.dir().join(format!("{id}.toml"))) {
            Ok(raw) => Ok(raw),
            Err(crate::DefinitionError::NotFound(_)) => crate::roles::builtin(id)
                .map(|c| new_document(&c).to_string())
                .ok_or_else(|| AuthoringError::new("NOT_FOUND", format!("role {id:?}"))),
            Err(e) => Err(e.into()),
        }
    }

    pub fn role_edit(&self, id: &str) -> Result<RoleEdit, AuthoringError> {
        let root = self.dir().parent().unwrap_or(self.dir());
        let locks = crate::write::locks(root, &format!("role:{id}"));
        let _reference_guard = locks.references.read().unwrap_or_else(|e| e.into_inner());
        let _guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        self.role_edit_locked(id)
    }
    fn role_edit_locked(&self, id: &str) -> Result<RoleEdit, AuthoringError> {
        let raw = self.source(id)?;
        Ok(RoleEdit {
            card: self.read(id)?,
            etag: crate::authoring::etag(&raw),
        })
    }

    pub fn create(&self, mut card: RoleCard) -> Result<RoleEdit, AuthoringError> {
        crate::roles::check_role_id(&card.id)
            .map_err(|e| AuthoringError::new("INVALID_AGENT_CARD", e.to_string()))?;
        let root = self.dir().parent().unwrap_or(self.dir());
        let locks = crate::write::locks(root, &format!("role:{}", card.id));
        let _reference_guard = locks.references.read().unwrap_or_else(|e| e.into_inner());
        let _guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.dir().join(format!("{}.toml", card.id));
        if crate::roles::builtin(&card.id).is_some() || std::fs::symlink_metadata(&path).is_ok() {
            return Err(AuthoringError::new("ALREADY_EXISTS", "role already exists"));
        }
        if card.kind.trim().is_empty() {
            card.kind = "specialist".into();
        }
        if card.display_name.is_empty() {
            card.display_name = card.id.clone();
        }
        card.status = "active".into();
        card.builtin = false;
        card.notes.clear();
        let raw = new_document(&card).to_string();
        crate::write::atomic(&path, raw.as_bytes())?;
        Ok(RoleEdit {
            card,
            etag: crate::authoring::etag(&raw),
        })
    }

    pub fn edit(
        &self,
        id: &str,
        expected_etag: &str,
        next: RoleCard,
    ) -> Result<RoleEdit, AuthoringError> {
        let root = self.dir().parent().unwrap_or(self.dir());
        let locks = crate::write::locks(root, &format!("role:{id}"));
        let _reference_guard = locks.references.read().unwrap_or_else(|e| e.into_inner());
        let _guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        self.edit_locked(id, expected_etag, next, None)
    }
    /// Change a card and append its note in one revision-checked publication.
    pub fn edit_with_note(
        &self,
        id: &str,
        expected_etag: &str,
        next: RoleCard,
        actor: &str,
        text: &str,
    ) -> Result<RoleEdit, AuthoringError> {
        if text.trim().is_empty() {
            return Err(AuthoringError::new(
                "INVALID_AGENT_CARD",
                "note text is empty",
            ));
        }
        let root = self.dir().parent().unwrap_or(self.dir());
        let locks = crate::write::locks(root, &format!("role:{id}"));
        let _reference_guard = locks.references.read().unwrap_or_else(|e| e.into_inner());
        let _guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        self.edit_locked(id, expected_etag, next, Some((actor, text)))
    }
    fn edit_locked(
        &self,
        id: &str,
        expected_etag: &str,
        mut next: RoleCard,
        note: Option<(&str, &str)>,
    ) -> Result<RoleEdit, AuthoringError> {
        let current = self.role_edit_locked(id)?;
        next.builtin = current.card.builtin;
        if next.display_name != current.card.display_name {
            next.display_name = next.display_name.trim().into();
        }
        if next.summary != current.card.summary {
            next.summary = next.summary.trim().into();
        }
        if next.kind != current.card.kind {
            next.kind = next.kind.trim().into();
            if next.kind.is_empty() {
                next.kind = "specialist".into();
            }
        }
        if expected_etag.is_empty() {
            return Err(AuthoringError::new(
                "PRECONDITION_REQUIRED",
                "role ETag is required",
            ));
        }
        if expected_etag != current.etag {
            return Err(AuthoringError::new("CONFLICT", "role changed"));
        }
        if next.id != id
            || next.kind.trim().is_empty()
            || !["", "active", "retired"].contains(&next.status.as_str())
            || next.notes != current.card.notes
        {
            return Err(AuthoringError::new(
                "INVALID_AGENT_CARD",
                "invalid role card",
            ));
        }
        if next == current.card && note.is_none() {
            return Ok(current);
        }
        let raw = self.source(id)?;
        let mut doc: DocumentMut = raw.parse().map_err(|e: toml_edit::TomlError| {
            AuthoringError::new("INVALID_AGENT_CARD", e.to_string())
        })?;
        let has_extra = doc["card"].as_table().is_some_and(|card| {
            card.iter().any(|(key, _)| {
                !["id", "display_name", "kind", "summary", "tags", "status"].contains(&key)
            })
        });
        if note.is_none()
            && !has_extra
            && crate::roles::builtin(id).is_some_and(|builtin| builtin == next)
        {
            let path = self.dir().join(format!("{id}.toml"));
            if std::fs::symlink_metadata(&path).is_ok() {
                std::fs::remove_file(path)?;
            }
            return self.role_edit_locked(id);
        }
        let table = doc["card"]
            .as_table_mut()
            .ok_or_else(|| AuthoringError::new("INVALID_AGENT_CARD", "missing card"))?;
        for (key, before, after) in [
            (
                "display_name",
                &current.card.display_name,
                &next.display_name,
            ),
            ("kind", &current.card.kind, &next.kind),
            ("summary", &current.card.summary, &next.summary),
            ("status", &current.card.status, &next.status),
        ] {
            if before != after {
                crate::write::set(table, key, value(after.clone()));
            }
        }
        if current.card.tags != next.tags {
            let mut tags = Array::new();
            for tag in &next.tags {
                tags.push(tag.as_str());
            }
            crate::write::set(table, "tags", value(tags));
        }
        if let Some((actor, text)) = note {
            let entry = crate::RoleNote {
                ts: crate::authoring::now()?,
                actor: if actor.trim().is_empty() {
                    crate::DEFAULT_ACTOR.into()
                } else {
                    actor.trim().into()
                },
                text: text.into(),
            };
            crate::authoring_create::append(&mut doc, "note", &entry)?;
        }
        let raw = doc.to_string();
        crate::write::atomic(&self.dir().join(format!("{id}.toml")), raw.as_bytes())?;
        Ok(RoleEdit {
            card: self.read(id)?,
            etag: crate::authoring::etag(&raw),
        })
    }
    /// Append an attributed note without replacing earlier notes.
    pub fn add_note(
        &self,
        id: &str,
        expected_etag: &str,
        actor: &str,
        text: &str,
    ) -> Result<RoleEdit, AuthoringError> {
        if text.is_empty() {
            return Err(AuthoringError::new(
                "INVALID_AGENT_CARD",
                "note text is empty",
            ));
        }
        let root = self.dir().parent().unwrap_or(self.dir());
        let locks = crate::write::locks(root, &format!("role:{id}"));
        let _reference_guard = locks.references.read().unwrap_or_else(|e| e.into_inner());
        let _guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        let current = self.role_edit_locked(id)?;
        if expected_etag.is_empty() {
            return Err(AuthoringError::new(
                "PRECONDITION_REQUIRED",
                "role ETag is required",
            ));
        }
        if expected_etag != current.etag {
            return Err(AuthoringError::new("CONFLICT", "role changed"));
        }
        let raw = self.source(id)?;
        let mut doc: DocumentMut = raw.parse().map_err(|e: toml_edit::TomlError| {
            AuthoringError::new("INVALID_AGENT_CARD", e.to_string())
        })?;
        let actor = actor.trim();
        let note = crate::RoleNote {
            ts: crate::authoring::now()?,
            actor: if actor.is_empty() {
                crate::DEFAULT_ACTOR.into()
            } else {
                actor.into()
            },
            text: text.into(),
        };
        crate::authoring_create::append(&mut doc, "note", &note)?;
        let raw = doc.to_string();
        crate::write::atomic(&self.dir().join(format!("{id}.toml")), raw.as_bytes())?;
        Ok(RoleEdit {
            card: self.read(id)?,
            etag: crate::authoring::etag(&raw),
        })
    }
    pub fn retire(&self, id: &str, etag: &str) -> Result<RoleEdit, AuthoringError> {
        self.status(id, etag, "retired")
    }
    pub fn restore(&self, id: &str, etag: &str) -> Result<RoleEdit, AuthoringError> {
        self.status(id, etag, "active")
    }
    fn status(&self, id: &str, etag: &str, status: &str) -> Result<RoleEdit, AuthoringError> {
        let root = self.dir().parent().unwrap_or(self.dir());
        let locks = crate::write::locks(root, &format!("role:{id}"));
        let _reference_guard = locks.references.read().unwrap_or_else(|e| e.into_inner());
        let _guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        let mut card = self.role_edit_locked(id)?.card;
        card.status = status.into();
        self.edit_locked(id, etag, card, None)
    }
    pub fn delete(&self, id: &str, expected_etag: &str) -> Result<(), AuthoringError> {
        let root = self.dir().parent().unwrap_or(self.dir());
        let locks = crate::write::locks(root, &format!("role:{id}"));
        let _reference_guard = locks.references.write().unwrap_or_else(|e| e.into_inner());
        let _guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        let report = self.usage_locked(id)?;
        if !report.usage.is_empty() {
            return Err(AuthoringError::new(
                "ROLE_IN_USE",
                format!(
                    "role {id:?} staffs {} slot{}: {}; restaff them, or retire the role instead",
                    report.usage.len(),
                    if report.usage.len() == 1 { "" } else { "s" },
                    report
                        .usage
                        .iter()
                        .map(|usage| format!(
                            "{} › {} › {}",
                            if usage.mission_title.is_empty() {
                                &usage.mission_slug
                            } else {
                                &usage.mission_title
                            },
                            if usage.formation_title.is_empty() {
                                &usage.formation_id
                            } else {
                                &usage.formation_title
                            },
                            if usage.slot_label.is_empty() {
                                &usage.slot_id
                            } else {
                                &usage.slot_label
                            }
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        if !report.complete {
            return Err(AuthoringError::new(
                "ROLE_USAGE_INCOMPLETE",
                "some missions cannot be read",
            ));
        }
        let current = self.role_edit_locked(id)?;
        if expected_etag.is_empty() {
            return Err(AuthoringError::new(
                "PRECONDITION_REQUIRED",
                "role ETag is required",
            ));
        }
        if expected_etag != current.etag {
            return Err(AuthoringError::new("CONFLICT", "role changed"));
        }
        let path = self.dir().join(format!("{id}.toml"));
        if std::fs::symlink_metadata(&path).is_err() && current.card.builtin {
            return Err(AuthoringError::new(
                "BUILTIN_ROLE",
                "built-in role has no card",
            ));
        }
        std::fs::remove_file(path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn card_edit_and_note_commit_together_and_reject_stale_etag() {
        let dir = std::env::temp_dir().join(format!("parvi-98-role-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = RoleStore::new(&dir);
        let original = store
            .create(RoleCard {
                id: "writer".into(),
                ..RoleCard::default()
            })
            .unwrap();
        let mut changed = original.card.clone();
        changed.summary = "Writes".into();
        let saved = store
            .edit_with_note(
                "writer",
                &original.etag,
                changed.clone(),
                "agent:test",
                "Keep context",
            )
            .unwrap();
        assert_eq!(saved.card.summary, "Writes");
        assert_eq!(saved.card.notes[0].text, "Keep context");
        assert_eq!(
            store
                .edit_with_note("writer", &original.etag, changed, "agent:test", "stale")
                .unwrap_err()
                .code(),
            "CONFLICT"
        );
        assert_eq!(store.read("writer").unwrap().notes.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
