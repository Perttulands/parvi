//! Construct graph nodes with Archon's default shape.
use crate::{AuthoringError, End, Formation, Gate, InputCard, Mission, Patch, Port, Slot};
use serde::Serialize;
use toml_edit::{ArrayOfTables, DocumentMut, Item};

pub(crate) fn append<T: Serialize>(
    doc: &mut DocumentMut,
    key: &str,
    node: &T,
) -> Result<(), AuthoringError> {
    let table = toml_edit::ser::to_document(node)
        .map_err(|e| AuthoringError::new("INVALID_DEFINITION", e.to_string()))?
        .into_table();
    crate::write::table_arrays(doc, &[key]);
    if !doc.contains_key(key) {
        doc[key] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let array = doc[key].as_array_of_tables_mut().ok_or_else(|| {
        AuthoringError::new(
            "INVALID_DEFINITION",
            format!("{key} is not an array of tables"),
        )
    })?;
    array.push(table);
    Ok(())
}

pub(crate) fn file_refs(files: &mut Vec<String>) -> Result<(), AuthoringError> {
    for file in files.iter_mut() {
        *file = file.trim().into();
    }
    files.retain(|file| !file.is_empty());
    for file in files.iter() {
        let windows_absolute = file.starts_with("\\\\")
            || (file.len() >= 3
                && file.as_bytes()[0].is_ascii_alphabetic()
                && file.as_bytes()[1] == b':'
                && matches!(file.as_bytes()[2], b'/' | b'\\'));
        if !std::path::Path::new(file).is_absolute() && !windows_absolute {
            return Err(AuthoringError::new(
                "RELATIVE_FILE_REFERENCE",
                format!("file {file:?} is relative: use an absolute path"),
            ));
        }
    }
    Ok(())
}

pub(crate) fn apply(
    doc: &mut DocumentMut,
    _mission: &Mission,
    patch: Patch,
) -> Result<crate::LayoutNode, AuthoringError> {
    match patch {
        Patch::CreateInputCard {
            title,
            goal,
            mut files,
            human_channel,
            x,
            y,
        } => {
            file_refs(&mut files)?;
            if !["", "notify", "session"].contains(&human_channel.as_str()) {
                return Err(AuthoringError::new(
                    "INVALID_HUMAN_CHANNEL",
                    "human channel must be notify or session",
                ));
            }
            let node = InputCard {
                id: crate::authoring::id("inp")?,
                title: if title.is_empty() {
                    "Input".into()
                } else {
                    title
                },
                goal,
                files,
                human_channel: if human_channel == "notify" {
                    String::new()
                } else {
                    human_channel
                },
                ..InputCard::default()
            };
            append(doc, "inputCard", &node)?;
            Ok(crate::LayoutNode { id: node.id, x, y })
        }
        Patch::CreateFormation {
            mut kind,
            mut title,
            x,
            y,
        } => {
            if kind.is_empty() {
                kind = "solo".into();
            }
            let slots: &[(&str, bool)] = match kind.as_str() {
                "solo" => &[("Agent", false)],
                "peer" => &[("Peer", false), ("Peer", false)],
                "orchestrated" => &[("Orchestrator", true), ("Agent", false), ("Agent", false)],
                _ => {
                    return Err(AuthoringError::new(
                        "UNSUPPORTED_FORMATION_TYPE",
                        "use solo, peer or orchestrated",
                    ));
                }
            };
            if title.is_empty() {
                title = match kind.as_str() {
                    "solo" => "Solo task",
                    "peer" => "Peer huddle",
                    _ => "Orchestration",
                }
                .into();
            }
            let mut node = Formation {
                id: crate::authoring::id("fmn")?,
                kind,
                title,
                ..Formation::default()
            };
            node.inputs.push(Port {
                id: crate::authoring::id("port")?,
                label: "Input".into(),
                ..Port::default()
            });
            node.outputs.push(Port {
                id: crate::authoring::id("port")?,
                label: "Output".into(),
                ..Port::default()
            });
            for (label, controller) in slots {
                node.slots.push(Slot {
                    id: crate::authoring::id("slot")?,
                    label: (*label).into(),
                    controller: *controller,
                    ..Slot::default()
                });
            }
            append(doc, "formation", &node)?;
            Ok(crate::LayoutNode { id: node.id, x, y })
        }
        Patch::CreateGate {
            title,
            mut kinds,
            criterion,
            check,
            check_version,
            check_value,
            files,
            x,
            y,
        } => {
            if kinds.is_empty() {
                kinds.push("formation".into());
            }
            let mut node = Gate {
                id: crate::authoring::id("gate")?,
                title: if title.is_empty() {
                    "Review gate".into()
                } else {
                    title
                },
                kinds,
                criterion,
                check,
                check_version,
                check_value,
                files,
            };
            crate::authoring_controls::check_gate(&mut node)?;
            append(doc, "gate", &node)?;
            Ok(crate::LayoutNode { id: node.id, x, y })
        }
        Patch::CreateEnd {
            mut title,
            mut outcome,
            x,
            y,
        } => {
            outcome = outcome.trim().into();
            title = title.trim().into();
            if outcome.is_empty() {
                outcome = "done".into();
            }
            if !["done", "rejected"].contains(&outcome.as_str()) {
                return Err(AuthoringError::new(
                    "INVALID_END_OUTCOME",
                    "outcome must be done or rejected",
                ));
            }
            if title.is_empty() {
                title = if outcome == "done" {
                    "Done"
                } else {
                    "Rejected"
                }
                .into();
            }
            let node = End {
                id: crate::authoring::id("end")?,
                title,
                outcome,
            };
            append(doc, "end", &node)?;
            Ok(crate::LayoutNode { id: node.id, x, y })
        }
        _ => unreachable!(),
    }
}

pub(crate) fn layout(
    store: &crate::Store,
    slug: &str,
    mission: &Mission,
    node: crate::LayoutNode,
) -> Result<(std::path::PathBuf, String), AuthoringError> {
    let path = store
        .root()
        .join("layout")
        .join(format!("{slug}.layout.toml"));
    let mut doc = match crate::links::read_definition(&path) {
        Ok(raw) => {
            let previous = crate::Layout::parse(&raw)?;
            if previous.mission_id != mission.id {
                return Err(AuthoringError::new(
                    "CONFLICT",
                    "layout belongs to another mission",
                ));
            }
            raw.parse::<DocumentMut>()
                .map_err(|e| AuthoringError::new("INVALID_DEFINITION", e.to_string()))?
        }
        Err(crate::DefinitionError::NotFound(_)) => {
            let mut doc = DocumentMut::new();
            doc["schema"] = toml_edit::value(1);
            doc
        }
        Err(err) => return Err(err.into()),
    };
    crate::write::table_arrays(&mut doc, &["node"]);
    crate::write::set(
        doc.as_table_mut(),
        "missionId",
        toml_edit::value(mission.id.clone()),
    );
    crate::write::set(
        doc.as_table_mut(),
        "missionRev",
        toml_edit::value(mission.rev),
    );
    crate::write::set(
        doc.as_table_mut(),
        "updatedAt",
        toml_edit::value(mission.updated_at.clone()),
    );
    append(&mut doc, "node", &node)?;
    let raw = doc.to_string();
    crate::Layout::parse(&raw)?;
    Ok((path, raw))
}
