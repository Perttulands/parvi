//! Formation titles, staffing and controller selection.
use crate::Error;
use parvi_mission_model::{Mission, Patch};
use toml_edit::{DocumentMut, Item, TableLike, Value, value};
fn not_found() -> Error {
    Error::new("not_found", "formation or slot")
}
pub(crate) fn formation_table<'a>(
    doc: &'a mut DocumentMut,
    id: &str,
) -> Result<&'a mut toml_edit::Table, Error> {
    doc["formation"]
        .as_array_of_tables_mut()
        .and_then(|arr| {
            arr.iter_mut()
                .find(|t| t.get("id").and_then(Item::as_str) == Some(id))
        })
        .ok_or_else(not_found)
}
pub(crate) fn slot_table<'a>(item: &'a mut Item, id: &str) -> Result<&'a mut dyn TableLike, Error> {
    match item {
        Item::ArrayOfTables(tables) => tables
            .iter_mut()
            .find(|t| t.get("id").and_then(Item::as_str) == Some(id))
            .map(|t| t as &mut dyn TableLike)
            .ok_or_else(not_found),
        Item::Value(Value::Array(array)) => array
            .iter_mut()
            .filter_map(Value::as_inline_table_mut)
            .find(|t| t.get("id").and_then(Value::as_str) == Some(id))
            .map(|t| t as &mut dyn TableLike)
            .ok_or_else(not_found),
        _ => Err(not_found()),
    }
}
pub(crate) fn set(table: &mut dyn TableLike, key: &str, next: &str) {
    if next.is_empty() {
        table.remove(key);
    } else if table.get(key).and_then(Item::as_str) != Some(next) {
        crate::write::set(table, key, value(next));
    }
}
/// A brief's work item, if any, must be a reference in a written form.
pub(crate) fn check_work(work: &str) -> Result<(), Error> {
    if work.is_empty() {
        return Ok(());
    }
    parvi_work_ref::parse(work)
        .map(|_| ())
        .map_err(|invalid| Error::new("invalid_bead_id", invalid.to_string()))
}
pub(crate) fn apply(doc: &mut DocumentMut, mission: &Mission, patch: Patch) -> Result<bool, Error> {
    match patch {
        Patch::SetBrief {
            formation_id,
            mut brief,
        } => {
            crate::authoring_create::file_refs(&mut brief.files)?;
            check_work(&brief.work)?;
            let formation = mission.formation(&formation_id).ok_or_else(not_found)?;
            if formation.brief.as_ref() == Some(&brief) {
                return Ok(false);
            }
            let next = toml_edit::ser::to_document(&brief)
                .map_err(|e| Error::new("invalid_definition", e.to_string()))?
                .into_table();
            formation_table(doc, &formation_id)?["brief"] = Item::Table(next);
        }
        Patch::ClearBrief { formation_id } => {
            let formation = mission.formation(&formation_id).ok_or_else(not_found)?;
            if formation.brief.is_none() {
                return Err(not_found());
            }
            formation_table(doc, &formation_id)?.remove("brief");
        }
        Patch::EditFormation {
            id,
            title,
            mut brief,
        } => {
            if let Some(brief) = &mut brief {
                crate::authoring_create::file_refs(&mut brief.files)?;
                check_work(&brief.work)?;
            }
            let formation = mission.formation(&id).ok_or_else(not_found)?;
            if formation.title == title && formation.brief == brief {
                return Ok(false);
            }
            let table = formation_table(doc, &id)?;
            crate::write::set(table, "title", value(title));
            if let Some(brief) = brief {
                let next = toml_edit::ser::to_document(&brief)
                    .map_err(|e| Error::new("invalid_definition", e.to_string()))?
                    .into_table();
                table["brief"] = Item::Table(next);
            } else {
                table.remove("brief");
            }
        }
        Patch::UpdateFormation { id, title, device } => {
            let formation = mission.formation(&id).ok_or_else(not_found)?;
            let title = title.filter(|t| *t != formation.title);
            let device = device
                .map(|d| d.trim().to_string())
                .filter(|d| *d != formation.device);
            if title.is_none() && device.is_none() {
                return Ok(false);
            }
            let table = formation_table(doc, &id)?;
            if let Some(title) = title {
                // An empty title is an explicit value, not an absent field.
                crate::write::set(table, "title", value(title));
            }
            match device.as_deref() {
                Some("") => {
                    table.remove("device");
                }
                Some(d) => crate::write::set(table, "device", value(d)),
                None => {}
            }
        }
        Patch::AssignSlot {
            formation_id,
            slot_id,
            agent_id,
            harness,
            model,
            effort,
        } => {
            let formation = mission.formation(&formation_id).ok_or_else(not_found)?;
            let slot = formation
                .slots
                .iter()
                .find(|s| s.id == slot_id)
                .ok_or_else(not_found)?;
            let (agent_id, harness, model, effort) =
                (agent_id.trim(), harness.trim(), model.trim(), effort.trim());
            if !agent_id.is_empty() && crate::roles::check_role_id(agent_id).is_err() {
                return Err(Error::new("invalid_slot_settings", "invalid role id"));
            }
            if [agent_id, harness, model, effort]
                .iter()
                .any(|s| !s.is_empty())
            {
                crate::Harnesses::from_host()
                    .check("slot", harness, model, effort)
                    .map_err(|e| Error::new("invalid_slot_settings", e.message))?;
            }
            if (
                slot.agent_id.as_str(),
                slot.harness.as_str(),
                slot.model.as_str(),
                slot.effort.as_str(),
            ) == (agent_id, harness, model, effort)
            {
                return Ok(false);
            }
            let table = formation_table(doc, &formation_id)?;
            let item = table.get_mut("slot").ok_or_else(not_found)?;
            let slot = slot_table(item, &slot_id)?;
            for (key, val) in [
                ("agentId", agent_id),
                ("harness", harness),
                ("model", model),
                ("effort", effort),
            ] {
                set(slot, key, val);
            }
        }
        Patch::SwapSlots {
            from_formation,
            from_slot,
            to_formation,
            to_slot,
        } => {
            let find = |formation: &str, slot: &str| {
                mission
                    .formation(formation)
                    .and_then(|f| f.slots.iter().find(|s| s.id == slot))
                    .ok_or_else(not_found)
            };
            let from = find(&from_formation, &from_slot)?;
            let to = find(&to_formation, &to_slot)?;
            let settings = |s: &parvi_mission_model::Slot| {
                [
                    s.agent_id.clone(),
                    s.harness.clone(),
                    s.model.clone(),
                    s.effort.clone(),
                ]
            };
            let first = settings(from);
            let second = settings(to);
            if first == second {
                return Ok(false);
            }
            for (formation, id, values) in [
                (&from_formation, &from_slot, &second),
                (&to_formation, &to_slot, &first),
            ] {
                let table = formation_table(doc, formation)?;
                let slot = slot_table(table.get_mut("slot").ok_or_else(not_found)?, id)?;
                for (key, value) in ["agentId", "harness", "model", "effort"].iter().zip(values) {
                    set(slot, key, value);
                }
            }
        }
        Patch::MakeController {
            formation_id,
            slot_id,
        } => {
            let formation = mission.formation(&formation_id).ok_or_else(not_found)?;
            if formation.kind != "orchestrated" {
                return Err(Error::new(
                    "invalid_controller_role",
                    "formation is not orchestrated",
                ));
            }
            if !formation.slots.iter().any(|s| s.id == slot_id) {
                return Err(not_found());
            }
            if formation
                .slots
                .iter()
                .all(|s| s.controller == (s.id == slot_id))
            {
                return Ok(false);
            }
            let table = formation_table(doc, &formation_id)?;
            let item = table.get_mut("slot").ok_or_else(not_found)?;
            for slot in &formation.slots {
                crate::write::set(
                    slot_table(item, &slot.id)?,
                    "controller",
                    value(slot.id == slot_id),
                );
            }
        }
        _ => unreachable!(),
    }
    Ok(true)
}
