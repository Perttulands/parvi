//! Translate CLI authoring commands into one node edit.
use super::{
    Args, Side, endpoint, only_slot, select, selected_input, set_node_flag, slot_id, value_flags,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub(super) fn operation<'a>(
    noun: &str,
    verb: &str,
    a: &Args,
    m: &Value,
) -> Result<(&'a str, Value, Option<&'a str>)> {
    let node = |list: &str| -> Result<String> { select(m, list, a.arg(1)?) };
    let mut v = value_flags(
        a,
        &[
            ("title", "title"),
            ("goal", "goal"),
            ("human-channel", "humanChannel"),
            ("input-hint", "inputHint"),
            ("criterion", "criterion"),
            ("check", "check"),
            ("check-version", "checkVersion"),
            ("check-value", "checkValue"),
            ("outcome", "outcome"),
            ("target", "target"),
            ("when-spent", "whenSpent"),
        ],
    );
    set_node_flag(&mut v, a, "files");
    for coord in ["x", "y"] {
        if let Some(n) = a.flag(coord) {
            v[coord] = json!(n.parse::<i64>()?);
        }
    }
    let result = match (noun, verb) {
        ("mission", "create") => ("createInputCard", Some("inputCard")),
        ("mission", "update") => {
            v["id"] = json!(selected_input(m, a.pos.get(1).map(String::as_str))?);
            ("updateInputCard", None)
        }
        ("mission", "input" | "output") => {
            let id = selected_input(m, None)?;
            let card = m["inputCards"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["id"] == id)
                .unwrap();
            let key = if verb == "input" { "inputs" } else { "outputs" };
            let mut items = card[key].as_array().cloned().unwrap_or_default();
            let name = a.arg(1)?;
            if a.has("required") && a.has("optional") {
                bail!("give --required or --optional, not both")
            }
            if a.has("delete")
                && ["kind", "description", "from", "required", "optional"]
                    .iter()
                    .any(|k| a.has(k))
            {
                bail!("--delete takes no other input or output flags")
            }
            let index = items.iter().position(|x| x["name"] == name);
            if a.has("delete") {
                let index = index.context("input or output not found")?;
                items.remove(index);
            } else {
                let mut item = index
                    .map(|i| items[i].clone())
                    .unwrap_or(json!({"name":name,"kind":"text","required":false}));
                for (flag, key) in [
                    ("kind", "kind"),
                    ("description", "description"),
                    ("from", "from"),
                ] {
                    if let Some(x) = a.flag(flag) {
                        item[key] = json!(x)
                    }
                }
                if a.has("required") {
                    item["required"] = json!(true)
                }
                if a.has("optional") {
                    item["required"] = json!(false)
                }
                if let Some(index) = index {
                    items[index] = item
                } else {
                    items.push(item)
                }
            }
            v = json!({"id":id,key:items});
            ("updateInputCard", None)
        }
        ("formation", "create") => {
            v["type"] = json!(a.pos.get(1).map(String::as_str).unwrap_or(""));
            ("createFormation", Some("formation"))
        }
        ("formation", "delete") => {
            v = json!({"id":node("formations")?});
            ("deleteFormation", None)
        }
        ("formation", "rename") => {
            v = json!({"id":node("formations")?,"title":a.arg(2)?});
            ("updateFormation", None)
        }
        ("formation", "place") => {
            // device/environment as the step editor offers it; home or empty
            // for wherever the run starts.
            let place = match a.arg(2)? {
                "home" => "",
                place => place,
            };
            if !place.is_empty() && !place.contains('/') {
                bail!("place is device/environment, or home");
            }
            v = json!({"id":node("formations")?,"device":place});
            ("updateFormation", None)
        }
        ("formation", "set-type") => {
            let id = node("formations")?;
            let keep = a
                .flag("keep-slot")
                .map(|s| slot_id(m, &id, s))
                .transpose()?
                .unwrap_or_default();
            v = json!({"id":id,"type":a.arg(2)?,"keepSlotId":keep});
            ("setFormationType", None)
        }
        ("formation", "set-slots") => {
            let id = node("formations")?;
            let formation = m["formations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["id"] == id)
                .unwrap();
            let controller = a.flag("controller").unwrap_or("");
            let mut slots = Vec::new();
            for entry in a.flags.get("slot").into_iter().flatten() {
                let (slot_id, label) = entry.split_once('=').context("--slot needs id=label")?;
                if slot_id.is_empty() || label.is_empty() {
                    bail!("--slot needs id=label");
                }
                let mut slot = formation["slots"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|s| s["id"] == slot_id)
                    .cloned()
                    .unwrap_or(json!({"id":slot_id}));
                slot["label"] = json!(label);
                slot["controller"] = json!(slot_id == controller);
                slots.push(slot);
            }
            if slots.is_empty() {
                bail!("at least one --slot is required");
            }
            if !controller.is_empty() && !slots.iter().any(|s| s["id"] == controller) {
                bail!("--controller must name a slot");
            }
            v = json!({"id":id,"type":formation["type"],"slots":slots});
            ("setFormationType", None)
        }
        ("formation", "assign" | "unassign") => {
            let id = node("formations")?;
            let slot = match a.flag("slot") {
                Some(selector) => slot_id(m, &id, selector)?,
                None => only_slot(m, &id)?,
            };
            v = json!({"formationId":id,"slotId":slot,"agentId":a.flag("role").unwrap_or(""),"harness":a.flag("harness").unwrap_or(""),"model":a.flag("model").unwrap_or(""),"effort":a.flag("effort").unwrap_or("")});
            ("assignSlot", None)
        }
        ("formation", "set-brief") => {
            if let Some(repository) = a.flag("repository") {
                v["repository"] = json!(repository);
            }
            v["formationId"] = json!(node("formations")?);
            if let Some(bead) = a.flag("bead") {
                v["beadId"] = json!(bead);
            }
            if let Some(links) = a.flags.get("link") {
                v["links"] = json!(links);
            }
            ("setBrief", None)
        }
        ("formation", "add-input" | "add-output") => {
            v = json!({"formationId":node("formations")?,"direction":if verb=="add-input"{"input"}else{"output"},"label":a.flag("label").context("--label is required")?,"kind":a.flag("kind").unwrap_or("")});
            ("addPort", None)
        }
        ("formation", "set-port") => {
            let formation = node("formations")?;
            let full = endpoint(m, &format!("{formation}:{}", a.arg(2)?), Side::From)?;
            v["formationId"] = json!(formation);
            v["portId"] = json!(full.split_once(':').unwrap().1);
            for (flag, key) in [("label", "label"), ("kind", "kind")] {
                if let Some(x) = a.flag(flag) {
                    v[key] = json!(x)
                }
            }
            ("updatePort", None)
        }
        ("formation", "wire") => {
            v = json!({"from":endpoint(m,a.arg(1)?,Side::From)?,"to":endpoint(m,a.arg(2)?,Side::To)?,"joinIfOccupied":a.has("join")});
            ("wireConnection", None)
        }
        ("mission", "wire") => {
            let (input, to) = if a.pos.len() == 2 {
                (selected_input(m, None)?, a.arg(1)?)
            } else {
                (selected_input(m, Some(a.arg(1)?))?, a.arg(2)?)
            };
            v = json!({"from":format!("{input}:out"),"to":endpoint(m,to,Side::To)?});
            ("wireConnection", None)
        }
        ("formation", "unwire") => {
            v = json!({"from":endpoint(m,a.arg(1)?,Side::From)?,"to":endpoint(m,a.arg(2)?,Side::To)?});
            ("unwireConnection", None)
        }
        ("gate", "create") => {
            if let Some(kinds) = a.flag("kinds") {
                v["kinds"] = json!(
                    kinds
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                )
            }
            ("createGate", Some("gate"))
        }
        ("gate", "update") => {
            v["id"] = json!(node("gates")?);
            if a.has("clear-check") {
                if ["check", "check-version", "check-value"]
                    .iter()
                    .any(|flag| a.has(flag))
                {
                    bail!("--clear-check cannot be combined with check fields");
                }
                for key in ["check", "checkVersion", "checkValue"] {
                    v[key] = json!("");
                }
            }
            if let Some(kinds) = a.flag("kinds") {
                v["kinds"] = json!(
                    kinds
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                )
            }
            ("updateGate", None)
        }
        ("gate", "delete") => {
            v = json!({"id":node("gates")?});
            ("deleteGate", None)
        }
        ("gate", "judge") => {
            if a.has("detach") {
                v = json!({"gateId":node("gates")?});
                ("detachGateJudge", None)
            } else {
                let chain = a
                    .flag("chain")
                    .context("--chain is required")?
                    .split(',')
                    .map(|s| select(m, "formations", s))
                    .collect::<Result<Vec<_>>>()?;
                v = json!({"gateId":node("gates")?,"chain":chain});
                ("setGateJudge", None)
            }
        }
        ("end", "create") => ("createEnd", Some("end")),
        ("end", "update") => {
            v["id"] = json!(node("ends")?);
            ("updateEnd", None)
        }
        ("end", "delete") => {
            v = json!({"id":node("ends")?});
            ("deleteEnd", None)
        }
        ("limit", "create" | "update") => {
            if verb == "update" {
                v["id"] = json!(node("limits")?)
            }
            if let Some(target) = a.flag("target") {
                v["target"] = json!(if target.is_empty() {
                    String::new()
                } else if target == "input" {
                    selected_input(m, None)?
                } else if let Ok(id) = select(m, "formations", target) {
                    id
                } else {
                    selected_input(m, Some(target))?
                });
            }
            for (flag, key) in [
                ("rounds", "rounds"),
                ("tokens", "tokens"),
                ("time", "seconds"),
                ("warn", "warnSeconds"),
            ] {
                if let Some(s) = a.flag(flag) {
                    v[key] = json!(duration(s)?);
                }
            }
            (
                if verb == "create" {
                    "createLimit"
                } else {
                    "updateLimit"
                },
                if verb == "create" {
                    Some("limit")
                } else {
                    None
                },
            )
        }
        ("limit", "delete") => {
            v = json!({"id":node("limits")?});
            ("deleteLimit", None)
        }
        _ => bail!("{noun} {verb} is not implemented"),
    };
    Ok((result.0, v, result.1))
}
fn duration(s: &str) -> Result<i64> {
    if s.is_empty() {
        return Ok(0);
    }
    let mut sum: i64 = 0;
    let mut digits = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            digits.push(c)
        } else {
            let n: i64 = digits.parse()?;
            digits.clear();
            let multiplier = match c {
                's' => 1,
                'm' => 60,
                'h' => 3600,
                _ => bail!("invalid duration"),
            };
            sum = sum
                .checked_add(n.checked_mul(multiplier).context("duration too large")?)
                .context("duration too large")?;
        }
    }
    if !digits.is_empty() {
        sum = sum
            .checked_add(digits.parse::<i64>()?)
            .context("duration too large")?
    }
    Ok(sum)
}
