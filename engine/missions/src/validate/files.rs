//! Reference files on Input cards, briefs and gates that name no file this
//! node can open. A file may still appear before a run reads it, so these
//! warn rather than block.

use std::path::Path;

use super::possessive;
use crate::finding::{Code, Finding};
use crate::mission::Mission;

pub fn findings(mission: &Mission) -> Vec<Finding> {
    let cards = mission
        .input_cards
        .iter()
        .map(|card| (&card.id, &card.files));
    let briefs = (mission.formations.iter()).filter_map(|formation| {
        formation
            .brief
            .as_ref()
            .map(|brief| (&formation.id, &brief.files))
    });
    let gates = mission.gates.iter().map(|gate| (&gate.id, &gate.files));
    let mut findings = Vec::new();
    for (node, files) in cards.chain(briefs).chain(gates) {
        for file in files {
            let (code, problem) = if !Path::new(file).is_absolute() {
                (Code::RelativeFile, "is relative: use an absolute path")
            } else if std::fs::metadata(file)
                .is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound)
            {
                (Code::MissingFile, "does not exist")
            } else {
                continue;
            };
            findings.push(Finding {
                path: Some(file.clone()),
                ..Finding::new(
                    code,
                    node,
                    format!(
                        "{} file {file} {problem}",
                        possessive(mission.node_name(node))
                    ),
                )
            });
        }
    }
    findings
}
