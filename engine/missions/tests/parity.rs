//! Validation emits the finding codes Archon emits on the same files.
//!
//! `tests/parity` is a state directory: missions, a retired role card and a
//! Codex model cache. `archon-findings.json` holds what Archon's own
//! `mission validate --json` reported on each mission, recorded by
//! `archon-findings.sh`; this test holds Parvi to every code and node.

use std::collections::BTreeMap;
use std::path::Path;

use parvi_mission_model::Finding;
use parvi_missions::{Harnesses, Store, validate};
use serde::Deserialize;

#[derive(Deserialize)]
struct Expected {
    errors: Vec<(String, String)>,
    warnings: Vec<(String, String)>,
}

fn located(findings: &[Finding]) -> Vec<(String, String)> {
    let mut pairs: Vec<_> = (findings.iter())
        .map(|finding| (finding.code.as_str().to_owned(), finding.node_id.clone()))
        .collect();
    pairs.sort();
    pairs
}

#[test]
fn findings_match_archons_on_every_parity_mission() {
    let parity = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
    let expected: BTreeMap<String, Expected> = serde_json::from_str(
        &std::fs::read_to_string(parity.join("archon-findings.json")).unwrap(),
    )
    .unwrap();
    let store = Store::new(&parity);
    let harnesses = Harnesses::from_codex_home(&parity.join("codex"));
    let listed: Vec<_> = store
        .list_missions()
        .unwrap()
        .into_iter()
        .map(|listing| listing.slug)
        .collect();
    assert_eq!(
        listed,
        expected.keys().cloned().collect::<Vec<_>>(),
        "every parity mission has Archon's findings"
    );
    for (slug, archon) in expected {
        let mut mission = store.read_mission(&slug).unwrap();
        // The recorded fixture uses Unix absolute paths. Preserve its existing,
        // missing and relative cases using paths native to the test host.
        if slug == "cards" {
            let existing = std::env::temp_dir();
            let missing = existing.join(format!("parvi-135-absent-{}", std::process::id()));
            assert!(!missing.exists());
            for files in mission
                .input_cards
                .iter_mut()
                .map(|c| &mut c.files)
                .chain(
                    mission
                        .formations
                        .iter_mut()
                        .filter_map(|f| f.brief.as_mut().map(|b| &mut b.files)),
                )
                .chain(mission.gates.iter_mut().map(|g| &mut g.files))
            {
                for file in files {
                    if file == "/" {
                        *file = existing.to_string_lossy().into_owned();
                    } else if file.starts_with("/nonexistent/") {
                        *file = missing
                            .join(file.trim_start_matches('/'))
                            .to_string_lossy()
                            .into_owned();
                    }
                }
            }
        }
        let report = validate(&mission, &store.roles(), &harnesses);
        let mut sorted = archon.errors.clone();
        sorted.sort();
        assert_eq!(located(&report.errors), sorted, "{slug} errors");
        let mut sorted = archon.warnings.clone();
        sorted.sort();
        assert_eq!(located(&report.warnings), sorted, "{slug} warnings");
    }
}

/// Archon's own examples load and validate clean, in the copies Archon ships.
#[test]
fn archons_examples_load_and_validate_clean() {
    let parity = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
    let store = Store::new(&parity);
    for slug in ["delivery", "operations-proof"] {
        let mission = store.read_mission(slug).unwrap();
        assert_eq!(mission.input_cards.len(), 1);
        let report = validate(&mission, &store.roles(), &Harnesses::default());
        assert_eq!(report, Default::default(), "{slug}");
    }
}
