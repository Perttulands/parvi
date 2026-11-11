//! Wiring checks and the join-on-wire operation.
use crate::Error;
use parvi_mission_model::{Connection, Mission, Patch, Port};
use toml_edit::{DocumentMut, Item};

fn endpoint<'a>(mission: &Mission, endpoint: &'a str, output: bool) -> Option<&'a str> {
    let direction = if output {
        crate::graph::Direction::Output
    } else {
        crate::graph::Direction::Input
    };
    crate::graph::endpoint_allows(mission, endpoint, direction)
        .then(|| crate::graph::endpoint_node(endpoint))
}

pub(crate) fn apply(doc: &mut DocumentMut, mission: &Mission, patch: Patch) -> Result<(), Error> {
    match patch {
        Patch::RewireConnection {
            from,
            previous_to,
            to,
            join_if_occupied,
            remove_previous_input,
        } => {
            let old = mission
                .connections
                .iter()
                .find(|c| c.from == from && c.to == previous_to)
                .ok_or_else(|| Error::new("not_found", "connection"))?;
            let mut remaining = mission.clone();
            remaining
                .connections
                .retain(|c| c.from != from || c.to != previous_to);
            apply(
                doc,
                mission,
                Patch::UnwireConnection {
                    from: from.clone(),
                    to: previous_to.clone(),
                },
            )?;
            apply(
                doc,
                &remaining,
                Patch::WireConnection {
                    id: old.id.clone(),
                    from,
                    to: to.clone(),
                    join_if_occupied,
                },
            )?;
            if remove_previous_input {
                let published = Mission::parse(&doc.to_string())?;
                let actual_to = published
                    .connections
                    .iter()
                    .find(|c| c.id == old.id)
                    .ok_or_else(|| Error::new("not_found", "rewired connection"))?
                    .to
                    .as_str();
                if previous_to == actual_to
                    || remaining
                        .connections
                        .iter()
                        .any(|c| c.from == previous_to || c.to == previous_to)
                {
                    return Err(Error::new(
                        "input_occupied",
                        "previous input remains in use",
                    ));
                }
                let (formation_id, port_id) = previous_to
                    .split_once(':')
                    .ok_or_else(|| Error::new("not_found", "previous input"))?;
                if !remaining
                    .formations
                    .iter()
                    .any(|f| f.id == formation_id && f.inputs.iter().any(|p| p.id == port_id))
                {
                    return Err(Error::new("not_found", "previous input"));
                }
                let formation = doc["formation"]
                    .as_array_of_tables_mut()
                    .and_then(|a| {
                        a.iter_mut()
                            .find(|t| t.get("id").and_then(Item::as_str) == Some(formation_id))
                    })
                    .ok_or_else(|| Error::new("not_found", "formation"))?;
                let ports = formation
                    .get_mut("input")
                    .ok_or_else(|| Error::new("not_found", "previous input"))?;
                if let Some(tables) = ports.as_array_of_tables_mut() {
                    let index = tables
                        .iter()
                        .position(|t| t.get("id").and_then(Item::as_str) == Some(port_id))
                        .ok_or_else(|| Error::new("not_found", "previous input"))?;
                    tables.remove(index);
                } else if let Some(array) = ports.as_array_mut() {
                    let index = array
                        .iter()
                        .position(|t| {
                            t.as_inline_table()
                                .and_then(|t| t.get("id"))
                                .and_then(|v| v.as_str())
                                == Some(port_id)
                        })
                        .ok_or_else(|| Error::new("not_found", "previous input"))?;
                    array.remove(index);
                } else {
                    return Err(Error::new("not_found", "previous input"));
                }
            }
        }
        Patch::UnwireConnection { from, to } => {
            let tables = doc
                .get_mut("connection")
                .and_then(Item::as_array_of_tables_mut)
                .ok_or_else(|| Error::new("not_found", "connection"))?;
            let index = tables
                .iter()
                .position(|t| {
                    t.get("from").and_then(Item::as_str) == Some(&from)
                        && t.get("to").and_then(Item::as_str) == Some(&to)
                })
                .ok_or_else(|| Error::new("not_found", "connection"))?;
            tables.remove(index);
        }
        Patch::WireConnection {
            mut id,
            from,
            mut to,
            join_if_occupied,
        } => {
            let from_node = endpoint(mission, &from, true)
                .ok_or_else(|| Error::new("not_found", "source port"))?;
            let to_node = endpoint(mission, &to, false)
                .ok_or_else(|| Error::new("not_found", "target port"))?;
            if from_node == to_node {
                return Err(Error::new("self_wire", "a node cannot wire to itself"));
            }
            if mission
                .connections
                .iter()
                .any(|c| c.from == from && c.to == to)
            {
                return Err(Error::new("duplicate_connection", "wire already exists"));
            }
            let fail = |s: &str| crate::graph::is_gate_fail(mission, s);
            let occupied = mission.end(to_node).is_none()
                && mission
                    .connections
                    .iter()
                    .any(|c| c.to == to && !fail(&from) && !fail(&c.from));
            if occupied {
                if !join_if_occupied || mission.formation(to_node).is_none() {
                    return Err(Error::new("input_occupied", "input already has a wire"));
                }
                let formation = doc["formation"]
                    .as_array_of_tables_mut()
                    .and_then(|a| {
                        a.iter_mut()
                            .find(|t| t.get("id").and_then(Item::as_str) == Some(to_node))
                    })
                    .ok_or_else(|| Error::new("not_found", "formation"))?;
                let port = Port {
                    id: crate::authoring::id("port")?,
                    label: "Input".into(),
                    ..Port::default()
                };
                let table = toml_edit::ser::to_document(&port)
                    .map_err(|e| Error::new("invalid_port", e.to_string()))?
                    .into_table();
                let inputs = formation
                    .get_mut("input")
                    .ok_or_else(|| Error::new("invalid_port", "formation inputs"))?;
                if let Some(tables) = inputs.as_array_of_tables_mut() {
                    tables.push(table);
                } else if let Some(array) = inputs.as_array_mut() {
                    array.push(table.into_inline_table());
                } else {
                    return Err(Error::new("invalid_port", "formation inputs"));
                }
                to = format!("{to_node}:{}", port.id);
            }
            if id.is_empty() {
                id = crate::authoring::id("edge")?;
            } else if id == "." || id == ".." || id.contains(['/', '\\', '\0']) {
                return Err(Error::new("not_found", "invalid wire id"));
            } else if mission.connections.iter().any(|c| c.id == id) {
                return Err(Error::new("already_exists", "wire id"));
            }
            crate::authoring_create::append(doc, "connection", &Connection { id, from, to })?;
        }
        _ => unreachable!(),
    }
    Ok(())
}
