//! Staffing: every slot states a harness and effort a seat can start with,
//! and any role it names exists and is not retired.

use crate::finding::{Code, Finding};
use crate::harness::Harnesses;
use crate::mission::Mission;
use crate::roles::{RETIRED, RoleStore};

pub fn findings(mission: &Mission, roles: &RoleStore, harnesses: &Harnesses) -> Vec<Finding> {
    let mut findings = Vec::new();
    for formation in &mission.formations {
        let id = &formation.id;
        let mut add = |code, message| findings.push(Finding::new(code, id, message));
        for slot in &formation.slots {
            let name = slot.name();
            if !slot.staffed() {
                add(
                    Code::UnstaffedSlot,
                    format!(
                        "formation {id:?} slot {name} needs a harness and effort, and optionally a role"
                    ),
                );
                continue;
            }
            // What the seat runs comes first, so a harness no seat can start
            // is named whatever role the slot has.
            let subject = format!("slot {name}");
            if let Err(problem) =
                harnesses.check(&subject, &slot.harness, &slot.model, &slot.effort)
            {
                add(
                    Code::InvalidSlotSettings,
                    format!("formation {id:?} {}", problem.message),
                );
                continue;
            }
            if slot.agent_id.is_empty() {
                continue;
            }
            let role = &slot.agent_id;
            match roles.read(role) {
                Ok(card) if card.status == RETIRED => add(
                    Code::RetiredPersona,
                    format!(
                        "formation {id:?} slot {name} uses retired role {role:?}; staff it with another role or none, or bring the role back"
                    ),
                ),
                Ok(_) => {}
                Err(error) if error.code == "not_found" => add(
                    Code::UnavailablePersona,
                    format!("formation {id:?} slot {name} names unknown role {role:?}"),
                ),
                Err(err) => add(
                    Code::UnavailablePersona,
                    format!("formation {id:?} slot {name} cannot read role {role:?}: {err}"),
                ),
            }
        }
    }
    findings
}
