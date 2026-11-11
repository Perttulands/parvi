//! Formation types, their slot shapes, and slot ids.

use super::possessive;
use crate::finding::{Code, Finding};
use crate::mission::Mission;
use crate::nodes::{Formation, ORCHESTRATED, PEER, SOLO};

/// What a slot id may be, for messages: its seat's session is named after
/// it, and a relayed verdict names it.
const SLOT_ID_RULE: &str =
    "a letter or digit, then up to 63 letters, digits, underscores or hyphens";

pub fn valid_slot_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Whether `device` names an environment, `<device>/<environment>`.
pub fn valid_device(device: &str) -> bool {
    device.split_once('/').is_some_and(|(name, environment)| {
        !name.is_empty()
            && !name.contains(char::is_whitespace)
            && ["linux", "wsl", "windows"].contains(&environment)
    })
}

/// A step's `device` names an environment, `<device>/<environment>`; a
/// device alone could mean either of a Windows machine's two.
pub fn device_findings(mission: &Mission) -> Vec<Finding> {
    mission
        .formations
        .iter()
        .filter(|formation| !formation.device.is_empty() && !valid_device(&formation.device))
        .map(|formation| {
            Finding::new(
                Code::UnknownDevice,
                &formation.id,
                format!(
                    "formation {:?} runs on {:?}; name an environment as <device>/<linux|wsl|windows>, or clear it to run on the coordinator",
                    formation.id, formation.device
                ),
            )
        })
        .collect()
}

pub fn shape_findings(mission: &Mission) -> Vec<Finding> {
    let mut findings = Vec::new();
    for formation in &mission.formations {
        if ![SOLO, PEER, ORCHESTRATED].contains(&formation.kind.as_str()) {
            findings.push(Finding::new(
                Code::InvalidFormationType,
                &formation.id,
                format!(
                    "formation {:?} has unsupported type {:?}; change it to solo, peer or orchestrated, or delete it",
                    formation.id, formation.kind
                ),
            ));
        } else {
            findings.extend(slot_shape_finding(formation));
        }
    }
    findings
}

/// Solo has one slot; peer has two or more and no controller; orchestrated
/// has one controller and a worker beside it.
fn slot_shape_finding(formation: &Formation) -> Option<Finding> {
    let id = &formation.id;
    let problem = |code, message| Some(Finding::new(code, id, message));
    let slots = formation.slots.len();
    if slots == 0 {
        return problem(
            Code::FormationWithoutSlots,
            format!("formation {id:?} has no slots; add a slot and staff it"),
        );
    }
    let controllers = formation
        .slots
        .iter()
        .filter(|slot| slot.controller)
        .count();
    match formation.kind.as_str() {
        SOLO if slots != 1 => problem(
            Code::InvalidFormationShape,
            format!("solo formation {id:?} needs exactly one slot; it has {slots}"),
        ),
        PEER if slots < 2 || controllers != 0 => problem(
            Code::InvalidFormationShape,
            format!(
                "peer formation {id:?} needs at least two slots and no controllers; it has {slots} slots and {controllers} controllers"
            ),
        ),
        ORCHESTRATED if controllers != 1 => problem(
            Code::OrchestratedController,
            format!(
                "orchestrated formation {id:?} needs exactly one controller slot; it has {controllers}"
            ),
        ),
        ORCHESTRATED if slots < 2 => problem(
            Code::OrchestratedController,
            format!("orchestrated formation {id:?} needs a worker slot beside its controller"),
        ),
        _ => None,
    }
}

/// A slot id used by more than one slot, and one that breaks the slot id
/// rule. Two seats with one id would share a session name, and a verdict
/// relayed by that slot would name neither.
pub fn slot_id_findings(mission: &Mission) -> Vec<Finding> {
    let mut holders: Vec<(&str, Vec<&str>)> = Vec::new();
    for formation in &mission.formations {
        for slot in mission_slots(formation) {
            match holders.iter_mut().find(|(id, _)| *id == slot) {
                Some((_, formations)) => formations.push(&formation.id),
                None => holders.push((slot, vec![&formation.id])),
            }
        }
    }
    let mut findings = Vec::new();
    for (slot, formations) in holders
        .iter()
        .filter(|(_, formations)| formations.len() > 1)
    {
        let mut nodes: Vec<&str> = Vec::new();
        for node in formations {
            if !nodes.contains(node) {
                nodes.push(node);
            }
        }
        let described = match nodes.as_slice() {
            [only] => format!("{} slots of formation {only:?}", formations.len()),
            _ => {
                let quoted: Vec<_> = nodes.iter().map(|node| format!("{node:?}")).collect();
                format!("formations {}", quoted.join(" and "))
            }
        };
        for node in &nodes {
            findings.push(Finding::new(
                Code::DuplicateSlotId,
                node,
                format!("slot id {slot:?} is used by {described}; give every slot in the mission its own id"),
            ));
        }
    }
    for formation in &mission.formations {
        for slot in mission_slots(formation).filter(|slot| !valid_slot_id(slot)) {
            findings.push(Finding::new(
                Code::InvalidSlotId,
                &formation.id,
                format!(
                    "{} slot id {slot:?} is not a slot id Parvi can use: {SLOT_ID_RULE}; rename it in the mission file",
                    possessive(mission.node_name(&formation.id))
                ),
            ));
        }
    }
    findings
}

/// A formation's non-blank slot ids.
fn mission_slots(formation: &Formation) -> impl Iterator<Item = &str> {
    formation
        .slots
        .iter()
        .map(|slot| slot.id.as_str())
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::valid_slot_id;

    #[test]
    fn slot_ids_follow_the_rule() {
        assert!(valid_slot_id("w1"));
        assert!(valid_slot_id(&format!("a{}", "b".repeat(63))));
        assert!(!valid_slot_id(&format!("a{}", "b".repeat(64))));
        assert!(!valid_slot_id("_w"));
        assert!(!valid_slot_id("worker one"));
        assert!(!valid_slot_id("työ"));
    }
}
