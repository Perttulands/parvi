//! Partial Input card edits and declaration normalization.
use crate::{AuthoringError, Mission, MissionInput, MissionOutput};
use serde::Serialize;
use std::collections::HashSet;
use toml_edit::{DocumentMut, Item, value};

#[derive(Clone, Debug, Default)]
pub struct InputCardUpdate {
    pub id: String,
    pub title: Option<String>,
    pub goal: Option<String>,
    pub files: Option<Vec<String>>,
    pub input_hint: Option<String>,
    pub human_channel: Option<String>,
    pub inputs: Option<Vec<MissionInput>>,
    pub outputs: Option<Vec<MissionOutput>>,
}
fn normalize_name(
    name: &mut String,
    kind: &mut String,
    seen: &mut HashSet<String>,
    code: &'static str,
) -> Result<(), AuthoringError> {
    *name = name.trim().into();
    *kind = kind.trim().into();
    if kind.is_empty() {
        *kind = "text".into();
    }
    if !crate::validate::inputs::valid_name(name)
        || !["text", "file", "folder"].contains(&kind.as_str())
        || !seen.insert(name.clone())
    {
        return Err(AuthoringError::new(
            code,
            "invalid or duplicate name or kind",
        ));
    }
    Ok(())
}
fn list_value<T: Serialize>(values: &[T]) -> Result<Item, AuthoringError> {
    #[derive(Serialize)]
    struct Wrapper<'a, T> {
        list: &'a [T],
    }
    let mut doc = toml_edit::ser::to_document(&Wrapper { list: values })
        .map_err(|e| AuthoringError::new("INVALID_DEFINITION", e.to_string()))?;
    Ok(doc.remove("list").unwrap())
}
pub(crate) fn apply(
    doc: &mut DocumentMut,
    mission: &Mission,
    update: InputCardUpdate,
) -> Result<bool, AuthoringError> {
    let old = mission
        .input_cards
        .iter()
        .find(|c| c.id == update.id)
        .ok_or_else(|| AuthoringError::new("NOT_FOUND", "Input card"))?;
    let mut next = old.clone();
    if let Some(title) = update.title {
        next.title = title;
    }
    if let Some(goal) = update.goal {
        next.goal = goal;
    }
    if let Some(mut files) = update.files {
        crate::authoring_create::file_refs(&mut files)?;
        next.files = files;
    }
    if let Some(hint) = update.input_hint {
        next.input_hint = hint;
    }
    if let Some(channel) = update.human_channel {
        next.human_channel = match channel.trim() {
            "" | "notify" => "".into(),
            "session" => "session".into(),
            _ => {
                return Err(AuthoringError::new(
                    "INVALID_HUMAN_CHANNEL",
                    "human channel must be notify or session",
                ));
            }
        };
    }
    if let Some(mut inputs) = update.inputs {
        let mut seen = HashSet::new();
        for input in &mut inputs {
            input.description = input.description.trim().into();
            normalize_name(
                &mut input.name,
                &mut input.kind,
                &mut seen,
                "INVALID_MISSION_INPUT",
            )?;
        }
        next.inputs = inputs;
    }
    if let Some(mut outputs) = update.outputs {
        let mut seen = HashSet::new();
        for output in &mut outputs {
            output.description = output.description.trim().into();
            output.from = output.from.trim().into();
            normalize_name(
                &mut output.name,
                &mut output.kind,
                &mut seen,
                "INVALID_MISSION_OUTPUT",
            )?;
            if !output.from.is_empty() && crate::graph::split_endpoint(&output.from).is_none() {
                return Err(AuthoringError::new(
                    "INVALID_MISSION_OUTPUT",
                    "from must name a node and port",
                ));
            }
        }
        next.outputs = outputs;
    }
    if old == &next {
        return Ok(false);
    }
    let table = doc["inputCard"]
        .as_array_of_tables_mut()
        .and_then(|a| {
            a.iter_mut()
                .find(|t| t.get("id").and_then(Item::as_str) == Some(update.id.as_str()))
        })
        .ok_or_else(|| AuthoringError::new("NOT_FOUND", "Input card"))?;
    for (key, before, after, optional) in [
        ("title", &old.title, &next.title, false),
        ("goal", &old.goal, &next.goal, false),
        ("inputHint", &old.input_hint, &next.input_hint, true),
        (
            "humanChannel",
            &old.human_channel,
            &next.human_channel,
            true,
        ),
    ] {
        if before != after {
            if optional && after.is_empty() {
                table.remove(key);
            } else {
                crate::write::set(table, key, value(after.clone()));
            }
        }
    }
    if old.files != next.files {
        if next.files.is_empty() {
            table.remove("files");
        } else {
            crate::write::set(table, "files", list_value(&next.files)?);
        }
    }
    if old.inputs != next.inputs {
        if next.inputs.is_empty() {
            table.remove("inputs");
        } else {
            crate::write::set(table, "inputs", list_value(&next.inputs)?);
        }
    }
    if old.outputs != next.outputs {
        if next.outputs.is_empty() {
            table.remove("outputs");
        } else {
            crate::write::set(table, "outputs", list_value(&next.outputs)?);
        }
    }
    Ok(true)
}
