//! Change a formation's shape without silently discarding staffed slots.
use crate::{Error, Mission, Patch, Slot};
use std::collections::HashSet;
use toml_edit::{DocumentMut, Item, value};
fn invalid(message: &str) -> Error {
    Error::new("invalid_type_change", message)
}
fn valid_id(id: &str) -> bool {
    id.len() <= 64
        && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn new_slot(label: &str, controller: bool) -> Result<Slot, Error> {
    Ok(Slot {
        id: crate::authoring::id("slot")?,
        label: label.into(),
        controller,
        ..Slot::default()
    })
}
pub(crate) fn apply(doc: &mut DocumentMut, mission: &Mission, patch: Patch) -> Result<bool, Error> {
    let Patch::SetFormationType {
        id,
        kind,
        keep_slot_id,
        slots,
    } = patch
    else {
        unreachable!()
    };
    if !["solo", "peer", "orchestrated"].contains(&kind.as_str()) {
        return Err(Error::new(
            "unsupported_formation_type",
            "use solo, peer or orchestrated",
        ));
    }
    if !keep_slot_id.is_empty() && kind != "solo" {
        return Err(invalid("keepSlotId applies only to solo"));
    }
    let current = mission
        .formation(&id)
        .ok_or_else(|| Error::new("not_found", "formation"))?;
    let next = if let Some(snapshot) = slots {
        let mut seen = HashSet::new();
        let controllers = snapshot.iter().filter(|s| s.controller).count();
        if snapshot
            .iter()
            .any(|s| !valid_id(&s.id) || !seen.insert(s.id.clone()))
            || match kind.as_str() {
                "solo" => snapshot.len() != 1,
                "peer" => snapshot.len() < 2 || controllers != 0,
                _ => snapshot.len() < 2 || controllers != 1,
            }
        {
            return Err(invalid("slot snapshot does not fit formation type"));
        }
        if !keep_slot_id.is_empty()
            && (!current.slots.iter().any(|s| s.id == keep_slot_id)
                || snapshot[0].id != keep_slot_id)
        {
            return Err(invalid("keepSlotId must name the retained current slot"));
        }
        if current
            .slots
            .iter()
            .any(|s| s.staffed() && !seen.contains(&s.id))
            && keep_slot_id.is_empty()
        {
            return Err(Error::new(
                "slot_choice_required",
                "snapshot would remove a staffed slot",
            ));
        }
        if mission
            .formations
            .iter()
            .any(|f| f.id != id && f.slots.iter().any(|s| seen.contains(&s.id)))
        {
            return Err(invalid("slot id already used"));
        }
        snapshot
    } else {
        let mut slots = current.slots.clone();
        if !keep_slot_id.is_empty() && !slots.iter().any(|s| s.id == keep_slot_id) {
            return Err(invalid("slot to keep does not exist"));
        }
        match kind.as_str() {
            "solo" => {
                if slots.is_empty() {
                    slots.push(new_slot("Agent", false)?);
                }
                let staffed: Vec<_> = slots.iter().filter(|s| s.staffed()).collect();
                if keep_slot_id.is_empty() && staffed.len() > 1 {
                    return Err(Error::new(
                        "slot_choice_required",
                        "choose the staffed slot to keep",
                    ));
                }
                let keep = if !keep_slot_id.is_empty() {
                    &keep_slot_id
                } else if let Some(slot) = staffed.first() {
                    &slot.id
                } else {
                    &slots[0].id
                };
                let mut slot = slots.iter().find(|s| &s.id == keep).unwrap().clone();
                slot.controller = false;
                vec![slot]
            }
            "peer" => {
                for s in &mut slots {
                    s.controller = false;
                }
                while slots.len() < 2 {
                    slots.push(new_slot("Peer", false)?);
                }
                slots
            }
            _ => {
                if slots.is_empty() {
                    slots.push(new_slot("Orchestrator", true)?);
                    slots.push(new_slot("Agent", false)?);
                    slots.push(new_slot("Agent", false)?);
                }
                let controllers: Vec<_> = slots
                    .iter()
                    .filter(|s| s.controller)
                    .map(|s| s.id.clone())
                    .collect();
                let controller = if controllers.len() == 1 {
                    &controllers[0]
                } else {
                    &slots[0].id
                }
                .clone();
                for slot in &mut slots {
                    slot.controller = slot.id == controller;
                }
                if slots.len() == 1 {
                    slots.push(new_slot("Agent", false)?);
                }
                slots
            }
        }
    };
    if current.kind == kind && current.slots == next {
        return Ok(false);
    }
    let table = doc["formation"]
        .as_array_of_tables_mut()
        .and_then(|a| {
            a.iter_mut()
                .find(|t| t.get("id").and_then(Item::as_str) == Some(id.as_str()))
        })
        .ok_or_else(|| invalid("formation source"))?;
    crate::write::set(table, "type", value(kind));
    // Retain the source table of each kept slot, including its comments.
    let old = table.remove("slot");
    let mut source = std::collections::HashMap::new();
    if let Some(Item::ArrayOfTables(tables)) = old {
        for table in tables.iter() {
            if let Some(id) = table.get("id").and_then(Item::as_str) {
                source.insert(id.to_owned(), table.clone());
            }
        }
    } else if let Some(Item::Value(toml_edit::Value::Array(array))) = old {
        for val in array.iter() {
            if let Some(t) = val.as_inline_table()
                && let Some(id) = t.get("id").and_then(|v| v.as_str())
            {
                source.insert(id.to_owned(), t.clone().into_table());
            }
        }
    }
    let mut result = toml_edit::ArrayOfTables::new();
    for slot in &next {
        let mut table = source.remove(&slot.id).unwrap_or_else(|| {
            toml_edit::ser::to_document(slot)
                .expect("slot serializes")
                .into_table()
        });
        for (key, field) in [
            ("id", &slot.id),
            ("label", &slot.label),
            ("agentId", &slot.agent_id),
            ("harness", &slot.harness),
            ("model", &slot.model),
            ("effort", &slot.effort),
        ] {
            if field.is_empty() && !["id", "label"].contains(&key) {
                table.remove(key);
            } else if table.get(key).and_then(Item::as_str) != Some(field) {
                crate::write::set(&mut table, key, value(field.clone()));
            }
        }
        if table.get("controller").and_then(Item::as_bool) != Some(slot.controller) {
            crate::write::set(&mut table, "controller", value(slot.controller));
        }
        result.push(table);
    }
    table["slot"] = Item::ArrayOfTables(result);
    Ok(true)
}
