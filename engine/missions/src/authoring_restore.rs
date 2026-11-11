//! Restore a deleted node, its wires and its canvas position in one edit.
use crate::Error;
use parvi_mission_model::{Connection, LayoutNode, Mission, Patch, RestoredNode};
use serde::Serialize;
use std::collections::HashSet;
use toml_edit::{ArrayOfTables, DocumentMut, Item};
fn invalid(message: &str) -> Error {
    Error::new("invalid_node_restore", message)
}
fn put<T: Serialize>(
    doc: &mut DocumentMut,
    key: &str,
    node: &T,
    index: Option<usize>,
) -> Result<(), Error> {
    let table = toml_edit::ser::to_document(node)
        .map_err(|e| invalid(&e.to_string()))?
        .into_table();
    if !doc.contains_key(key) {
        doc[key] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let array = doc[key]
        .as_array_of_tables_mut()
        .ok_or_else(|| invalid("node list"))?;
    array.insert(index.unwrap_or(array.len()).min(array.len()), table);
    Ok(())
}
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}
pub(crate) fn apply(
    doc: &mut DocumentMut,
    mission: &Mission,
    mut node: RestoredNode,
    connections: Vec<Connection>,
    index: Option<usize>,
    x: i64,
    y: i64,
) -> Result<LayoutNode, Error> {
    let id = match &node {
        RestoredNode::InputCard(n) => &n.id,
        RestoredNode::Formation(n) => &n.id,
        RestoredNode::Gate(n) => &n.id,
        RestoredNode::End(n) => &n.id,
        RestoredNode::Limit(n) => &n.id,
    }
    .clone();
    if !valid_id(&id)
        || mission.input_cards.iter().any(|n| n.id == id)
        || mission.formation(&id).is_some()
        || mission.gate(&id).is_some()
        || mission.end(&id).is_some()
        || mission.limits.iter().any(|n| n.id == id)
    {
        return Err(invalid("node id is invalid or already taken"));
    }
    match &mut node {
        RestoredNode::InputCard(n) => {
            crate::authoring_create::file_refs(&mut n.files)?;
            put(doc, "inputCard", n, index)?;
            let current = Mission::parse(&doc.to_string())?;
            crate::authoring_input::apply(
                doc,
                &current,
                parvi_mission_model::InputCardUpdate {
                    id: n.id.clone(),
                    human_channel: Some(n.human_channel.clone()),
                    inputs: Some(n.inputs.clone()),
                    outputs: Some(n.outputs.clone()),
                    ..Default::default()
                },
            )?;
        }
        RestoredNode::Formation(n) => {
            if !["solo", "peer", "orchestrated"].contains(&n.kind.as_str()) {
                return Err(Error::new(
                    "unsupported_formation_type",
                    "invalid formation type",
                ));
            }
            let mut ports = HashSet::new();
            for port in n.inputs.iter().chain(&n.outputs) {
                if !valid_id(&port.id) || !ports.insert(&port.id) {
                    return Err(invalid("invalid or duplicate port id"));
                }
            }
            if n.inputs
                .iter()
                .any(|p| !["", "text"].contains(&p.kind.as_str()))
                || n.outputs
                    .iter()
                    .any(|p| !["", "text", "file", "folder"].contains(&p.kind.as_str()))
            {
                return Err(Error::new("invalid_port", "invalid port kind"));
            }
            for port in n.inputs.iter_mut().chain(&mut n.outputs) {
                if port.kind == "text" {
                    port.kind.clear();
                }
            }
            let controllers = n.slots.iter().filter(|s| s.controller).count();
            if match n.kind.as_str() {
                "solo" => n.slots.len() != 1,
                "peer" => n.slots.len() < 2 || controllers != 0,
                _ => n.slots.len() < 2 || controllers != 1,
            } {
                return Err(invalid("invalid formation shape"));
            }
            let mut seen = HashSet::new();
            for slot in &n.slots {
                if slot.id.is_empty()
                    || slot.id.len() > 64
                    || !slot.id.as_bytes()[0].is_ascii_alphanumeric()
                    || !slot
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                    || !seen.insert(&slot.id)
                    || mission
                        .formations
                        .iter()
                        .any(|f| f.slots.iter().any(|s| s.id == slot.id))
                {
                    return Err(invalid("invalid or duplicate slot id"));
                }
            }
            if let Some(brief) = &mut n.brief {
                crate::authoring_create::file_refs(&mut brief.files)?;
                crate::authoring_staff::check_work(&brief.work)?;
            }
            put(doc, "formation", n, index)?;
        }
        RestoredNode::Gate(n) => {
            crate::authoring_controls::check_gate(n)?;
            put(doc, "gate", n, index)?;
        }
        RestoredNode::End(n) => {
            n.outcome = n.outcome.trim().into();
            if n.outcome.is_empty() {
                n.outcome = "done".into();
            }
            if !["done", "rejected"].contains(&n.outcome.as_str()) {
                return Err(Error::new("invalid_end_outcome", "invalid End outcome"));
            }
            if n.title.trim().is_empty() {
                n.title = if n.outcome == "done" {
                    "Done"
                } else {
                    "Rejected"
                }
                .into();
            }
            put(doc, "end", n, index)?;
        }
        RestoredNode::Limit(n) => {
            if n.title.trim().is_empty() {
                n.title = "Limit".into();
            }
            if [n.rounds, n.seconds, n.warn_seconds, n.tokens]
                .into_iter()
                .flatten()
                .any(|v| v <= 0)
            {
                return Err(Error::new("invalid_limit", "limit knobs must be positive"));
            }
            put(doc, "limit", n, index)?;
        }
    }
    let mut current = Mission::parse(&doc.to_string())?;
    for mut wire in connections {
        if crate::graph::endpoint_node(&wire.from) != id.as_str()
            && crate::graph::endpoint_node(&wire.to) != id.as_str()
        {
            return Err(invalid("connection does not touch restored node"));
        }
        if !valid_id(&wire.id) || current.connections.iter().any(|c| c.id == wire.id) {
            wire.id = crate::authoring::id("edge")?;
        }
        crate::authoring_wire::apply(
            doc,
            &current,
            Patch::WireConnection {
                id: wire.id,
                from: wire.from,
                to: wire.to,
                join_if_occupied: false,
            },
        )
        .map_err(|e| {
            if ["not_found", "input_occupied"].contains(&e.code.as_str()) {
                invalid(&e.to_string())
            } else {
                e
            }
        })?;
        current = Mission::parse(&doc.to_string())?;
    }
    Ok(LayoutNode { id, x, y })
}
