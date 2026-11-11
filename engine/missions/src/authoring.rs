//! Revision-checked edits of mission definitions.
use crate::{Error, Store};
use parvi_mission_model::{Edit, Mission, MissionDeletion, Patch, Precondition};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use toml_edit::{DocumentMut, value};

pub(crate) fn etag(raw: &str) -> String {
    Sha256::digest(raw.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub(crate) fn now() -> Result<String, Error> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| Error::new("io", e.to_string()))
}
pub(crate) fn id(prefix: &str) -> Result<String, Error> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).map_err(|e| Error::new("io", e.to_string()))?;
    Ok(format!(
        "{prefix}_{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
fn path(store: &Store, slug: &str) -> PathBuf {
    store
        .root()
        .join("missions")
        .join(format!("{slug}.mission.toml"))
}
fn parse_document(raw: &str) -> Result<DocumentMut, Error> {
    raw.parse()
        .map_err(|e: toml_edit::TomlError| Error::new("invalid_definition", e.to_string()))
}

fn editable_arrays(patch: &Patch) -> &'static [&'static str] {
    match patch {
        Patch::Title(_) => &[],
        Patch::UpdateInputCard(_) | Patch::CreateInputCard { .. } => &["inputCard"],
        Patch::CreateFormation { .. }
        | Patch::SetBrief { .. }
        | Patch::ClearBrief { .. }
        | Patch::UpdateFormation { .. }
        | Patch::EditFormation { .. }
        | Patch::AssignSlot { .. }
        | Patch::SwapSlots { .. }
        | Patch::MakeController { .. }
        | Patch::SetFormationType { .. } => &["formation"],
        Patch::CreateGate { .. } => &["gate"],
        Patch::CreateEnd { .. } | Patch::UpdateEnd(_) => &["end"],
        Patch::CreateLimit { .. } | Patch::UpdateLimit(_) => &["limit"],
        Patch::SetGateJudge { .. } | Patch::DetachGateJudge { .. } | Patch::UpdateGate(_) => {
            &["gate", "connection"]
        }
        Patch::AddPort { .. }
        | Patch::UpdatePort { .. }
        | Patch::RemovePort { .. }
        | Patch::RestorePort { .. }
        | Patch::WireConnection { .. }
        | Patch::RewireConnection { .. }
        | Patch::UnwireConnection { .. } => &["formation", "connection"],
        Patch::DeleteInputCard { .. } => &["inputCard", "connection"],
        Patch::DeleteFormation { .. } => &["formation", "limit", "connection"],
        Patch::DeleteGate { .. } => &["gate", "connection"],
        Patch::DeleteEnd { .. } => &["end", "connection"],
        Patch::DeleteLimit { .. } => &["limit", "connection"],
        Patch::RestoreNode { .. } => &[
            "inputCard",
            "formation",
            "gate",
            "end",
            "limit",
            "connection",
        ],
    }
}
pub(crate) fn edit(raw: &str) -> Result<Edit, Error> {
    Ok(Edit {
        mission: Mission::parse(raw)?,
        etag: etag(raw),
    })
}
fn check(precondition: &Precondition, current: &Edit) -> Result<(), Error> {
    if precondition.rev == 0 || precondition.etag.is_empty() {
        return Err(Error::new(
            "precondition_required",
            "revision and ETag are required",
        ));
    }
    if precondition.rev != current.mission.rev || precondition.etag != current.etag {
        return Err(Error::new("conflict", "mission changed"));
    }
    Ok(())
}

impl Store {
    pub fn mission_edit(&self, selector: &str) -> Result<Edit, Error> {
        let slug = self.resolve(selector)?;
        let raw = crate::links::read_definition(&path(self, &slug))?;
        edit(&raw)
    }

    pub fn create_mission(&self, slug: &str, title: &str, actor: &str) -> Result<Edit, Error> {
        crate::store::check_slug(slug)?;
        let locks = crate::write::locks(self.root(), &format!("mission:{slug}"));
        let _reference_guard = locks.references.read().unwrap_or_else(|e| e.into_inner());
        let _document_guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        let target = path(self, slug);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => {
                return Err(Error::new("already_exists", "mission already exists"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(crate::error::io(error)),
        }
        let mut doc = DocumentMut::new();
        doc["schema"] = value(1);
        doc["id"] = value(id("msn")?);
        doc["slug"] = value(slug);
        doc["title"] = value(if title.trim().is_empty() {
            slug
        } else {
            title.trim()
        });
        crate::write::set(doc.as_table_mut(), "rev", toml_edit::value(1));
        if !actor.trim().is_empty() {
            crate::write::set(
                doc.as_table_mut(),
                "updatedBy",
                toml_edit::value(actor.trim()),
            );
        }
        crate::write::set(doc.as_table_mut(), "updatedAt", toml_edit::value(now()?));
        let raw = doc.to_string();
        let result = edit(&raw)?;
        crate::write::atomic(&target, raw.as_bytes()).map_err(crate::error::io)?;
        Ok(result)
    }

    pub fn patch(
        &self,
        selector: &str,
        precondition: Precondition,
        patch: Patch,
        actor: &str,
    ) -> Result<Edit, Error> {
        let scope = crate::write::locks(self.root(), "scope");
        let _reference_guard = scope.references.read().unwrap_or_else(|e| e.into_inner());
        let slug = self.resolve(selector)?;
        let locks = crate::write::locks(self.root(), &format!("mission:{slug}"));
        let _document_guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        if self.resolve(selector)? != slug {
            return Err(Error::new("conflict", "mission selector changed"));
        }
        let target = path(self, &slug);
        let raw = crate::links::read_definition(&target)?;
        let current = edit(&raw)?;
        check(&precondition, &current)?;
        let mut doc = parse_document(&raw)?;
        crate::write::table_arrays(&mut doc, editable_arrays(&patch));
        let layout_node = match patch {
            Patch::Title(title) => {
                if current.mission.title == title {
                    return Ok(current);
                }
                let decor = doc["title"].as_value().map(|v| v.decor().clone());
                let mut updated = value(title);
                if let (Some(decor), Some(v)) = (decor, updated.as_value_mut()) {
                    *v.decor_mut() = decor;
                }
                doc["title"] = updated;
                None
            }
            patch @ (Patch::SetGateJudge { .. } | Patch::DetachGateJudge { .. }) => {
                if !crate::authoring_judge::apply(&mut doc, &current.mission, patch)? {
                    return Ok(current);
                }
                None
            }
            patch @ Patch::CreateLimit { .. } => Some(crate::authoring_controls::create_limit(
                &mut doc,
                &current.mission,
                patch,
            )?),
            patch @ (Patch::UpdateLimit(_) | Patch::UpdateEnd(_) | Patch::UpdateGate(_)) => {
                if !crate::authoring_controls::update(&mut doc, &current.mission, patch)? {
                    return Ok(current);
                }
                None
            }
            Patch::RestoreNode {
                node,
                connections,
                index,
                x,
                y,
            } => Some(crate::authoring_restore::apply(
                &mut doc,
                &current.mission,
                node,
                connections,
                index,
                x,
                y,
            )?),
            patch @ (Patch::AddPort { .. }
            | Patch::UpdatePort { .. }
            | Patch::RemovePort { .. }
            | Patch::RestorePort { .. }) => {
                if !crate::authoring_port::apply(&mut doc, &current.mission, patch)? {
                    return Ok(current);
                }
                None
            }
            Patch::UpdateInputCard(update) => {
                if !crate::authoring_input::apply(&mut doc, &current.mission, update)? {
                    return Ok(current);
                }
                None
            }
            patch @ (Patch::UpdateFormation { .. }
            | Patch::EditFormation { .. }
            | Patch::AssignSlot { .. }
            | Patch::SwapSlots { .. }
            | Patch::MakeController { .. }
            | Patch::SetBrief { .. }
            | Patch::ClearBrief { .. }) => {
                if !crate::authoring_staff::apply(&mut doc, &current.mission, patch)? {
                    return Ok(current);
                }
                None
            }
            patch @ Patch::SetFormationType { .. } => {
                if !crate::authoring_type::apply(&mut doc, &current.mission, patch)? {
                    return Ok(current);
                }
                None
            }
            patch @ (Patch::WireConnection { .. }
            | Patch::UnwireConnection { .. }
            | Patch::RewireConnection { .. }) => {
                crate::authoring_wire::apply(&mut doc, &current.mission, patch)?;
                None
            }
            patch @ (Patch::DeleteInputCard { .. }
            | Patch::DeleteFormation { .. }
            | Patch::DeleteGate { .. }
            | Patch::DeleteEnd { .. }
            | Patch::DeleteLimit { .. }) => {
                return crate::authoring_delete::apply(
                    self, &slug, &target, doc, current, patch, actor,
                );
            }
            other => Some(crate::authoring_create::apply(
                &mut doc,
                &current.mission,
                other,
            )?),
        };
        crate::write::set(
            doc.as_table_mut(),
            "rev",
            toml_edit::value(current.mission.rev + 1),
        );
        if !actor.is_empty() {
            crate::write::set(doc.as_table_mut(), "updatedBy", toml_edit::value(actor));
        }
        crate::write::set(doc.as_table_mut(), "updatedAt", toml_edit::value(now()?));
        let raw = doc.to_string();
        let updated = edit(&raw)?;
        // A legacy definition may be repaired one unsupported node at a time.
        // Other edits cannot save a mission the runtime cannot execute.
        let old = crate::validate::unsupported(&current.mission);
        let unsupported = crate::validate::unsupported(&updated.mission);
        let repairing = unsupported.len() < old.len()
            && unsupported.iter().all(|finding| {
                old.iter().any(|existing| {
                    existing.code == finding.code && existing.node_id == finding.node_id
                })
            });
        if !repairing && let Some(finding) = unsupported.first() {
            return Err(Error::new(finding.code.as_str(), &finding.message));
        }
        let layout = if let Some(node) = layout_node {
            Some(crate::authoring_create::layout(
                self,
                &slug,
                &updated.mission,
                node,
            )?)
        } else {
            None
        };
        if let Some((path, layout_raw)) = layout {
            crate::write::pair((&target, raw.as_bytes()), (&path, layout_raw.as_bytes()))
                .map_err(crate::error::io)?;
        } else {
            crate::write::atomic(&target, raw.as_bytes()).map_err(crate::error::io)?;
        }
        Ok(updated)
    }
}

impl Store {
    pub fn delete_mission(
        &self,
        selector: &str,
        precondition: Precondition,
    ) -> Result<MissionDeletion, Error> {
        let scope = crate::write::locks(self.root(), "scope");
        let _reference_guard = scope.references.read().unwrap_or_else(|e| e.into_inner());
        let slug = self.resolve(selector)?;
        let locks = crate::write::locks(self.root(), &format!("mission:{slug}"));
        let _document_guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        if self.resolve(selector)? != slug {
            return Err(Error::new("conflict", "mission selector changed"));
        }
        let target = path(self, &slug);
        let raw = crate::links::read_definition(&target)?;
        let current = edit(&raw)?;
        check(&precondition, &current)?;
        let archive_id = id("archive")?;
        let layout = self
            .root()
            .join("layout")
            .join(format!("{slug}.layout.toml"));
        let archived_layout =
            layout.with_file_name(format!("{slug}.layout.toml.deleted-{archive_id}"));
        let archived_mission =
            target.with_file_name(format!("{slug}.mission.toml.deleted-{archive_id}"));
        let has_layout = match std::fs::symlink_metadata(&layout) {
            Ok(_) => {
                std::fs::rename(&layout, &archived_layout).map_err(crate::error::io)?;
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(crate::error::io(e)),
        };
        if let Err(error) = std::fs::rename(&target, &archived_mission) {
            if has_layout {
                std::fs::rename(&archived_layout, &layout).map_err(crate::error::io)?;
            }
            return Err(crate::error::io(error));
        }
        Ok(MissionDeletion {
            id: current.mission.id,
            slug,
            title: current.mission.title,
            archive_id,
        })
    }
}
