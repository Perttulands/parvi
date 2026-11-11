//! Remove a node, its wires and dependent Limit cards as one edit.
use crate::{Edit, Error, Patch, Store};
use std::collections::HashSet;
use std::path::Path;
use toml_edit::DocumentMut;

fn remove(doc: &mut DocumentMut, key: &str, ids: &HashSet<String>) -> bool {
    let Some(tables) = doc
        .get_mut(key)
        .and_then(|item| item.as_array_of_tables_mut())
    else {
        return false;
    };
    let mut indices = Vec::new();
    for (index, table) in tables.iter().enumerate() {
        if table
            .get("id")
            .and_then(|item| item.as_str())
            .is_some_and(|id| ids.contains(id))
        {
            indices.push(index);
        }
    }
    let found = !indices.is_empty();
    for index in indices.into_iter().rev() {
        tables.remove(index);
    }
    found
}

pub(crate) fn apply(
    store: &Store,
    slug: &str,
    target: &Path,
    mut doc: DocumentMut,
    current: Edit,
    patch: Patch,
    actor: &str,
) -> Result<Edit, Error> {
    let (key, id, remove_limits) = match patch {
        Patch::DeleteInputCard { id } => ("inputCard", id, false),
        Patch::DeleteFormation { id } => ("formation", id, true),
        Patch::DeleteGate { id } => ("gate", id, false),
        Patch::DeleteEnd { id } => ("end", id, false),
        Patch::DeleteLimit { id } => ("limit", id, false),
        _ => unreachable!(),
    };
    let mut ids = HashSet::from([id.clone()]);
    if !remove(&mut doc, key, &ids) {
        return Err(Error::new("not_found", format!("node {id:?}")));
    }
    if remove_limits {
        for limit in &current.mission.limits {
            if limit.target == id {
                ids.insert(limit.id.clone());
            }
        }
        remove(&mut doc, "limit", &ids);
    }
    let mut wires = HashSet::new();
    for wire in &current.mission.connections {
        if [&wire.from, &wire.to]
            .iter()
            .any(|endpoint| ids.contains(endpoint.split(':').next().unwrap_or("")))
        {
            wires.insert(wire.id.clone());
        }
    }
    remove(&mut doc, "connection", &wires);
    crate::write::set(
        doc.as_table_mut(),
        "rev",
        toml_edit::value(current.mission.rev + 1),
    );
    if !actor.is_empty() {
        crate::write::set(doc.as_table_mut(), "updatedBy", toml_edit::value(actor));
    }
    crate::write::set(
        doc.as_table_mut(),
        "updatedAt",
        toml_edit::value(crate::authoring::now()?),
    );
    let raw = doc.to_string();
    let updated = crate::authoring::edit(&raw)?;
    let layout_path = store
        .root()
        .join("layout")
        .join(format!("{slug}.layout.toml"));
    let layout = match crate::links::read_definition(&layout_path) {
        Ok(raw) => {
            let old = crate::Layout::parse(&raw)?;
            if old.mission_id != updated.mission.id {
                return Err(Error::new("conflict", "layout belongs to another mission"));
            }
            let mut doc: DocumentMut = raw.parse().map_err(|e: toml_edit::TomlError| {
                Error::new("invalid_definition", e.to_string())
            })?;
            crate::write::table_arrays(&mut doc, &["node"]);
            remove(&mut doc, "node", &ids);
            crate::write::set(
                doc.as_table_mut(),
                "missionRev",
                toml_edit::value(updated.mission.rev),
            );
            crate::write::set(
                doc.as_table_mut(),
                "updatedAt",
                toml_edit::value(updated.mission.updated_at.clone()),
            );
            let raw = doc.to_string();
            crate::Layout::parse(&raw)?;
            Some(raw)
        }
        Err(error) if error.code == "not_found" => None,
        Err(error) => return Err(error),
    };
    if let Some(layout_raw) = layout {
        crate::write::pair(
            (target, raw.as_bytes()),
            (&layout_path, layout_raw.as_bytes()),
        )
        .map_err(crate::error::io)?;
    } else {
        crate::write::atomic(target, raw.as_bytes()).map_err(crate::error::io)?;
    }
    Ok(updated)
}
