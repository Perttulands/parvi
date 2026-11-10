//! Revision-associated canvas positions and wire lanes.
use crate::{AuthoringError, Layout, LayoutEdge, LayoutNode, Store};
use toml_edit::{DocumentMut, value};

#[derive(Clone, Debug)]
pub enum LayoutPatch {
    Nodes(Vec<LayoutNode>),
    Edges(Vec<LayoutEdge>),
}
#[derive(Clone, Debug)]
pub struct LayoutEdit {
    pub layout: Layout,
    pub etag: String,
}

impl Store {
    pub fn layout_edit(&self, selector: &str) -> Result<LayoutEdit, AuthoringError> {
        let scope = crate::write::locks(self.root(), "scope");
        let _reference_guard = scope.references.read().unwrap_or_else(|e| e.into_inner());
        let slug = self.resolve(selector)?;
        let locks = crate::write::locks(self.root(), &format!("mission:{slug}"));
        let _document_guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        if self.resolve(selector)? != slug {
            return Err(AuthoringError::new("CONFLICT", "mission selector changed"));
        }
        self.layout_edit_locked(&slug)
    }
    fn layout_edit_locked(&self, slug: &str) -> Result<LayoutEdit, AuthoringError> {
        let mission = self.read_mission(slug)?;
        let path = self
            .root()
            .join("layout")
            .join(format!("{slug}.layout.toml"));
        match crate::links::read_definition(&path) {
            Ok(raw) => {
                let layout = Layout::parse(&raw)?;
                if layout.mission_id != mission.id {
                    return Err(AuthoringError::new(
                        "CONFLICT",
                        "layout belongs to another mission",
                    ));
                }
                Ok(LayoutEdit {
                    layout,
                    etag: crate::authoring::etag(&raw),
                })
            }
            Err(crate::DefinitionError::NotFound(_)) => Ok(LayoutEdit {
                layout: Layout {
                    schema: 1,
                    mission_id: mission.id,
                    mission_rev: mission.rev,
                    ..Layout::default()
                },
                etag: "*".into(),
            }),
            Err(e) => Err(e.into()),
        }
    }
    pub fn patch_layout(
        &self,
        selector: &str,
        expected_etag: &str,
        patch: LayoutPatch,
    ) -> Result<LayoutEdit, AuthoringError> {
        let scope = crate::write::locks(self.root(), "scope");
        let _reference_guard = scope.references.read().unwrap_or_else(|e| e.into_inner());
        let slug = self.resolve(selector)?;
        let locks = crate::write::locks(self.root(), &format!("mission:{slug}"));
        let _document_guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        if self.resolve(selector)? != slug {
            return Err(AuthoringError::new("CONFLICT", "mission selector changed"));
        }
        let mission = self.read_mission(&slug)?;
        let current = self.layout_edit_locked(&slug)?;
        if expected_etag.is_empty() {
            return Err(AuthoringError::new(
                "PRECONDITION_REQUIRED",
                "layout ETag is required",
            ));
        }
        if current.etag == "*" && (expected_etag != "*" || matches!(patch, LayoutPatch::Edges(_))) {
            return Err(AuthoringError::new("NOT_FOUND", "layout"));
        }
        if current.etag != expected_etag {
            return Err(AuthoringError::new("CONFLICT", "layout changed"));
        }
        let path = self
            .root()
            .join("layout")
            .join(format!("{slug}.layout.toml"));
        let mut doc: DocumentMut = if expected_etag == "*" {
            let mut doc = DocumentMut::new();
            doc["schema"] = value(1);
            doc
        } else {
            crate::links::read_definition(&path)?
                .parse()
                .map_err(|e: toml_edit::TomlError| {
                    AuthoringError::new("INVALID_DEFINITION", e.to_string())
                })?
        };
        crate::write::table_arrays(
            &mut doc,
            match &patch {
                LayoutPatch::Nodes(_) => &["node"],
                LayoutPatch::Edges(_) => &["edge"],
            },
        );
        match patch {
            LayoutPatch::Nodes(nodes) => {
                for node in nodes {
                    if node.id.is_empty() {
                        continue;
                    }
                    if let Some(table) = entry(&mut doc, "node", &node.id) {
                        crate::write::set(table, "x", value(node.x));
                        crate::write::set(table, "y", value(node.y));
                    } else {
                        crate::authoring_create::append(&mut doc, "node", &node)?;
                    }
                }
            }
            LayoutPatch::Edges(edges) => {
                for edge in edges {
                    if edge.id.is_empty() {
                        continue;
                    }
                    if let Some(table) = entry(&mut doc, "edge", &edge.id) {
                        crate::write::set(table, "lane", value(edge.lane));
                    } else {
                        crate::authoring_create::append(&mut doc, "edge", &edge)?;
                    }
                }
            }
        }
        crate::write::set(
            doc.as_table_mut(),
            "missionId",
            toml_edit::value(mission.id),
        );
        crate::write::set(
            doc.as_table_mut(),
            "missionRev",
            toml_edit::value(mission.rev),
        );
        crate::write::set(
            doc.as_table_mut(),
            "updatedAt",
            toml_edit::value(crate::authoring::now()?),
        );
        let raw = doc.to_string();
        let layout = Layout::parse(&raw)?;
        crate::write::atomic(&path, raw.as_bytes())?;
        Ok(LayoutEdit {
            layout,
            etag: crate::authoring::etag(&raw),
        })
    }
}

fn entry<'a>(doc: &'a mut DocumentMut, key: &str, id: &str) -> Option<&'a mut toml_edit::Table> {
    doc.get_mut(key)?
        .as_array_of_tables_mut()?
        .iter_mut()
        .find(|table| table.get("id").and_then(toml_edit::Item::as_str) == Some(id))
}
