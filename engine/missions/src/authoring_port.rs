//! Edit formation ports and restore their wire identities on undo.
use crate::{AuthoringError, Mission, Patch, Port};
use toml_edit::{DocumentMut, Item, Value, value};
fn missing() -> AuthoringError {
    AuthoringError::new("NOT_FOUND", "formation or port")
}
fn kind(direction: &str, raw: &str) -> Result<String, AuthoringError> {
    let kind = raw.trim();
    if !["", "text", "file", "folder"].contains(&kind)
        || (direction == "input" && !["", "text"].contains(&kind))
    {
        return Err(AuthoringError::new("INVALID_PORT", "invalid port kind"));
    }
    Ok(if kind == "text" { "" } else { kind }.into())
}
fn direction(s: &str) -> Result<&str, AuthoringError> {
    if ["input", "output"].contains(&s) {
        Ok(s)
    } else {
        Err(AuthoringError::new(
            "INVALID_PORT_DIRECTION",
            "direction must be input or output",
        ))
    }
}
fn insert(
    doc: &mut DocumentMut,
    formation_id: &str,
    direction: &str,
    port: &Port,
    index: Option<usize>,
) -> Result<(), AuthoringError> {
    let formation = crate::authoring_staff::formation_table(doc, formation_id)?;
    let mut table = toml_edit::ser::to_document(port)
        .map_err(|e| AuthoringError::new("INVALID_PORT", e.to_string()))?
        .into_table();
    table.set_implicit(false);
    let item = formation
        .entry(direction)
        .or_insert_with(|| value(toml_edit::Array::new()));
    if let Some(tables) = item.as_array_of_tables_mut() {
        let i = index.unwrap_or(tables.len()).min(tables.len());
        tables.insert(i, table);
    } else if let Some(array) = item.as_array_mut() {
        let i = index.unwrap_or(array.len()).min(array.len());
        array.insert(i, table.into_inline_table());
    } else {
        return Err(missing());
    }
    Ok(())
}
pub(crate) fn apply(
    doc: &mut DocumentMut,
    mission: &Mission,
    patch: Patch,
) -> Result<bool, AuthoringError> {
    match patch {
        Patch::AddPort {
            formation_id,
            direction: dir,
            mut label,
            kind: raw,
        } => {
            direction(&dir)?;
            if mission.formation(&formation_id).is_none() {
                return Err(missing());
            }
            if label.is_empty() {
                label = if dir == "input" { "Input" } else { "Output" }.into();
            }
            let port = Port {
                id: crate::authoring::id("port")?,
                label,
                kind: kind(&dir, &raw)?,
            };
            insert(doc, &formation_id, &dir, &port, None)?;
        }
        Patch::UpdatePort {
            formation_id,
            port_id,
            label,
            kind: raw,
        } => {
            let formation = mission.formation(&formation_id).ok_or_else(missing)?;
            let (dir, old) = formation
                .inputs
                .iter()
                .find(|p| p.id == port_id)
                .map(|p| ("input", p))
                .or_else(|| {
                    formation
                        .outputs
                        .iter()
                        .find(|p| p.id == port_id)
                        .map(|p| ("output", p))
                })
                .ok_or_else(missing)?;
            let label = label.map(|s| s.trim().to_owned());
            if label.as_ref().is_some_and(String::is_empty) {
                return Err(AuthoringError::new("INVALID_PORT", "a port needs a label"));
            }
            let next_kind = raw.map(|s| kind(dir, &s)).transpose()?;
            if label.as_ref().is_none_or(|s| s == &old.label)
                && next_kind.as_ref().is_none_or(|s| s == &old.kind)
            {
                return Ok(false);
            }
            let table = crate::authoring_staff::formation_table(doc, &formation_id)?;
            let port = crate::authoring_staff::slot_table(
                table.get_mut(dir).ok_or_else(missing)?,
                &port_id,
            )?;
            if let Some(label) = label {
                crate::authoring_staff::set(port, "label", &label);
            }
            if let Some(kind) = next_kind {
                crate::authoring_staff::set(port, "kind", &kind);
            }
        }
        Patch::RemovePort {
            formation_id,
            port_id,
        } => {
            let formation = mission.formation(&formation_id).ok_or_else(missing)?;
            let dir = if formation.inputs.iter().any(|p| p.id == port_id) {
                "input"
            } else if formation.outputs.iter().any(|p| p.id == port_id) {
                "output"
            } else {
                return Err(missing());
            };
            let table = crate::authoring_staff::formation_table(doc, &formation_id)?;
            let item = table.get_mut(dir).ok_or_else(missing)?;
            match item {
                Item::ArrayOfTables(a) => {
                    let i = a
                        .iter()
                        .position(|t| t.get("id").and_then(Item::as_str) == Some(port_id.as_str()))
                        .ok_or_else(missing)?;
                    a.remove(i);
                }
                Item::Value(Value::Array(a)) => {
                    let i = a
                        .iter()
                        .position(|t| {
                            t.as_inline_table()
                                .and_then(|t| t.get("id"))
                                .and_then(Value::as_str)
                                == Some(port_id.as_str())
                        })
                        .ok_or_else(missing)?;
                    a.remove(i);
                }
                _ => return Err(missing()),
            }
            let endpoint = format!("{formation_id}:{port_id}");
            if let Some(wires) = doc
                .get_mut("connection")
                .and_then(Item::as_array_of_tables_mut)
            {
                let indices: Vec<_> = wires
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| {
                        ["from", "to"]
                            .iter()
                            .any(|k| t.get(k).and_then(Item::as_str) == Some(endpoint.as_str()))
                    })
                    .map(|(i, _)| i)
                    .collect();
                for i in indices.into_iter().rev() {
                    wires.remove(i);
                }
            }
        }
        Patch::RestorePort {
            formation_id,
            direction: dir,
            mut port,
            index,
            connections,
        } => {
            direction(&dir)?;
            let formation = mission.formation(&formation_id).ok_or_else(missing)?;
            if !crate::authoring_restore::valid_id(&port.id)
                || formation
                    .inputs
                    .iter()
                    .chain(&formation.outputs)
                    .any(|p| p.id == port.id)
            {
                return Err(AuthoringError::new(
                    "INVALID_NODE_RESTORE",
                    "invalid or duplicate port",
                ));
            }
            port.kind = kind(&dir, &port.kind)?;
            insert(doc, &formation_id, &dir, &port, index)?;
            let mut current = Mission::parse(&doc.to_string())?;
            let endpoint = format!("{formation_id}:{}", port.id);
            for mut connection in connections {
                if connection.from != endpoint && connection.to != endpoint {
                    return Err(AuthoringError::new(
                        "INVALID_NODE_RESTORE",
                        "restored wire does not touch port",
                    ));
                }
                if !crate::authoring_restore::valid_id(&connection.id)
                    || current.connections.iter().any(|c| c.id == connection.id)
                {
                    connection.id = crate::authoring::id("edge")?;
                }
                crate::authoring_wire::apply(
                    doc,
                    &current,
                    Patch::WireConnection {
                        id: connection.id,
                        from: connection.from,
                        to: connection.to,
                        join_if_occupied: false,
                    },
                )
                .map_err(|e| {
                    if ["NOT_FOUND", "INPUT_OCCUPIED"].contains(&e.code()) {
                        AuthoringError::new("INVALID_NODE_RESTORE", e.to_string())
                    } else {
                        e
                    }
                })?;
                current = Mission::parse(&doc.to_string())?;
            }
        }
        _ => unreachable!(),
    }
    Ok(true)
}
