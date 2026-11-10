//! Revision-checked edits of mission definitions.
use crate::{AuthoringError, Mission, Store};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use toml_edit::{DocumentMut, value};

#[derive(Clone, Debug, Default)]
pub struct Precondition {
    pub rev: i64,
    pub etag: String,
}

#[derive(Clone, Debug)]
pub struct Edit {
    pub mission: Mission,
    pub etag: String,
}

/// A single board mutation. Further operations share the same checked write path.
#[derive(Clone, Debug)]
pub enum Patch {
    Title(String),
    SetGateJudge {
        gate_id: String,
        chain: Vec<String>,
    },
    DetachGateJudge {
        gate_id: String,
    },
    CreateLimit {
        limit: crate::Limit,
        x: i64,
        y: i64,
    },
    UpdateLimit(crate::LimitUpdate),
    UpdateEnd(crate::EndUpdate),
    UpdateGate(crate::GateUpdate),
    RestoreNode {
        node: crate::RestoredNode,
        connections: Vec<crate::Connection>,
        index: Option<usize>,
        x: i64,
        y: i64,
    },
    AddPort {
        formation_id: String,
        direction: String,
        label: String,
        kind: String,
    },
    UpdatePort {
        formation_id: String,
        port_id: String,
        label: Option<String>,
        kind: Option<String>,
    },
    RemovePort {
        formation_id: String,
        port_id: String,
    },
    RestorePort {
        formation_id: String,
        direction: String,
        port: crate::Port,
        index: Option<usize>,
        connections: Vec<crate::Connection>,
    },
    UpdateInputCard(crate::InputCardUpdate),
    SetBrief {
        formation_id: String,
        brief: crate::Brief,
    },
    ClearBrief {
        formation_id: String,
    },
    UpdateFormation {
        id: String,
        title: Option<String>,
        /// Where the step runs (#124); blank runs it on the coordinator.
        device: Option<String>,
    },
    EditFormation {
        id: String,
        title: String,
        brief: Option<crate::Brief>,
    },
    AssignSlot {
        formation_id: String,
        slot_id: String,
        agent_id: String,
        harness: String,
        model: String,
        effort: String,
    },
    SwapSlots {
        from_formation: String,
        from_slot: String,
        to_formation: String,
        to_slot: String,
    },
    MakeController {
        formation_id: String,
        slot_id: String,
    },
    SetFormationType {
        id: String,
        kind: String,
        keep_slot_id: String,
        slots: Option<Vec<crate::Slot>>,
    },
    WireConnection {
        id: String,
        from: String,
        to: String,
        join_if_occupied: bool,
    },
    RewireConnection {
        from: String,
        previous_to: String,
        to: String,
        join_if_occupied: bool,
        remove_previous_input: bool,
    },
    UnwireConnection {
        from: String,
        to: String,
    },
    DeleteInputCard {
        id: String,
    },
    DeleteFormation {
        id: String,
    },
    DeleteGate {
        id: String,
    },
    DeleteEnd {
        id: String,
    },
    DeleteLimit {
        id: String,
    },
    CreateInputCard {
        title: String,
        goal: String,
        files: Vec<String>,
        human_channel: String,
        x: i64,
        y: i64,
    },
    CreateFormation {
        kind: String,
        title: String,
        x: i64,
        y: i64,
    },
    CreateGate {
        title: String,
        kinds: Vec<String>,
        criterion: String,
        check: String,
        check_version: String,
        check_value: String,
        files: Vec<String>,
        x: i64,
        y: i64,
    },
    CreateEnd {
        title: String,
        outcome: String,
        x: i64,
        y: i64,
    },
}

pub(crate) fn etag(raw: &str) -> String {
    Sha256::digest(raw.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub(crate) fn now() -> Result<String, AuthoringError> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| AuthoringError::new("IO", e.to_string()))
}
pub(crate) fn id(prefix: &str) -> Result<String, AuthoringError> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).map_err(|e| AuthoringError::new("IO", e.to_string()))?;
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
fn parse_document(raw: &str) -> Result<DocumentMut, AuthoringError> {
    raw.parse()
        .map_err(|e: toml_edit::TomlError| AuthoringError::new("INVALID_DEFINITION", e.to_string()))
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
pub(crate) fn edit(raw: &str) -> Result<Edit, AuthoringError> {
    Ok(Edit {
        mission: Mission::parse(raw)?,
        etag: etag(raw),
    })
}
fn check(precondition: &Precondition, current: &Edit) -> Result<(), AuthoringError> {
    if precondition.rev == 0 || precondition.etag.is_empty() {
        return Err(AuthoringError::new(
            "PRECONDITION_REQUIRED",
            "revision and ETag are required",
        ));
    }
    if precondition.rev != current.mission.rev || precondition.etag != current.etag {
        return Err(AuthoringError::new("CONFLICT", "mission changed"));
    }
    Ok(())
}

impl Store {
    pub fn mission_edit(&self, selector: &str) -> Result<Edit, AuthoringError> {
        let slug = self.resolve(selector)?;
        let raw = crate::links::read_definition(&path(self, &slug))?;
        edit(&raw)
    }

    pub fn create_mission(
        &self,
        slug: &str,
        title: &str,
        actor: &str,
    ) -> Result<Edit, AuthoringError> {
        crate::store::check_slug(slug)?;
        let locks = crate::write::locks(self.root(), &format!("mission:{slug}"));
        let _reference_guard = locks.references.read().unwrap_or_else(|e| e.into_inner());
        let _document_guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        let target = path(self, slug);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => {
                return Err(AuthoringError::new(
                    "ALREADY_EXISTS",
                    "mission already exists",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
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
        crate::write::atomic(&target, raw.as_bytes())?;
        Ok(result)
    }

    pub fn patch(
        &self,
        selector: &str,
        precondition: Precondition,
        patch: Patch,
        actor: &str,
    ) -> Result<Edit, AuthoringError> {
        let scope = crate::write::locks(self.root(), "scope");
        let _reference_guard = scope.references.read().unwrap_or_else(|e| e.into_inner());
        let slug = self.resolve(selector)?;
        let locks = crate::write::locks(self.root(), &format!("mission:{slug}"));
        let _document_guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        if self.resolve(selector)? != slug {
            return Err(AuthoringError::new("CONFLICT", "mission selector changed"));
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
            crate::write::pair((&target, raw.as_bytes()), (&path, layout_raw.as_bytes()))?;
        } else {
            crate::write::atomic(&target, raw.as_bytes())?;
        }
        Ok(updated)
    }
}

/// An archived mission and the id shared by its archived layout.
#[derive(Clone, Debug)]
pub struct MissionDeletion {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub archive_id: String,
}

impl Store {
    pub fn delete_mission(
        &self,
        selector: &str,
        precondition: Precondition,
    ) -> Result<MissionDeletion, AuthoringError> {
        let scope = crate::write::locks(self.root(), "scope");
        let _reference_guard = scope.references.read().unwrap_or_else(|e| e.into_inner());
        let slug = self.resolve(selector)?;
        let locks = crate::write::locks(self.root(), &format!("mission:{slug}"));
        let _document_guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        if self.resolve(selector)? != slug {
            return Err(AuthoringError::new("CONFLICT", "mission selector changed"));
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
                std::fs::rename(&layout, &archived_layout)?;
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        if let Err(error) = std::fs::rename(&target, &archived_mission) {
            if has_layout {
                std::fs::rename(&archived_layout, &layout)?;
            }
            return Err(error.into());
        }
        Ok(MissionDeletion {
            id: current.mission.id,
            slug,
            title: current.mission.title,
            archive_id,
        })
    }
}
