//! Mission validation: every problem that would stop a run, so authoring
//! can accept drafts and a run start names what they still need.
//!
//! The finding codes and their rules are Archon's (`mission validate`, which
//! runs its admission check over the whole mission). Each check lives in its
//! own file; this one runs them and sorts the report.

mod files;
mod formations;
pub(crate) mod inputs;
mod limits;
mod nodes;
mod outputs;
mod staffing;
mod wiring;

use crate::harness::Harnesses;
use crate::roles::RoleStore;
use parvi_mission_model::{Mission, Report};

/// Validates a whole mission. Role cards come from `roles`, and what each
/// harness model accepts from `harnesses`.
pub fn validate(mission: &Mission, roles: &RoleStore, harnesses: &Harnesses) -> Report {
    let mut report = Report::default();
    let errors = &mut report.errors;
    errors.extend(wiring::connection_findings(mission));
    errors.extend(wiring::gate_findings(mission));
    errors.extend(formations::shape_findings(mission));
    errors.extend(formations::slot_id_findings(mission));
    errors.extend(formations::device_findings(mission));
    errors.extend(nodes::findings(mission));
    let (limit_errors, limit_warnings) = limits::findings(mission);
    errors.extend(limit_errors);
    errors.extend(wiring::route_findings(mission));
    errors.extend(inputs::findings(mission));
    errors.extend(outputs::port_kind_findings(mission));
    errors.extend(outputs::mission_output_findings(mission));
    errors.extend(wiring::input_card_findings(mission));
    errors.extend(staffing::findings(mission, roles, harnesses));

    let warnings = &mut report.warnings;
    warnings.extend(limit_warnings);
    warnings.extend(wiring::unreachable_findings(mission));
    warnings.extend(files::findings(mission));
    warnings.extend(wiring::not_runnable_findings(mission));
    report.sort();
    report
}

/// "Brief sign-off's", or "Research notes'" for a name ending in s.
fn possessive(name: &str) -> String {
    if name.ends_with(['s', 'S']) {
        format!("{name}'")
    } else {
        format!("{name}'s")
    }
}
