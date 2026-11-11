use parvi_missions::{AdmissionFailure, Coordinator, LabExecutor, RunSettings, RunStart};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let mut b = [0; 8];
        getrandom::fill(&mut b).unwrap();
        let p = std::env::temp_dir().join(format!("parvi-99-run-parity-{}", u64::from_ne_bytes(b)));
        std::fs::create_dir_all(p.join("missions")).unwrap();
        std::fs::create_dir_all(p.join("roles")).unwrap();
        std::fs::write(
            p.join("roles/critic.toml"),
            include_str!("parity/roles/critic.toml"),
        )
        .unwrap();
        std::fs::write(
            p.join("missions/hello.mission.toml"),
            include_str!("parity/runs/missions/solo-lab.mission.toml"),
        )
        .unwrap();
        std::fs::write(
            p.join("missions/admission-findings.mission.toml"),
            include_str!("parity/runs/missions/admission-findings.mission.toml"),
        )
        .unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn solo_lab_ledger_and_admission_findings_match_archon() {
    let root = Scratch::new();
    let coordinator =
        Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let request = RunStart {
        mission: "hello".into(),
        input_card_id: "inp_hello".into(),
        expected_rev: 1,
        inputs: BTreeMap::from([("brief".into(), "hello".into())]),
        ..Default::default()
    };
    let id = coordinator.start_mission(request).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let p = coordinator.run(&id).unwrap();
        if p.r#final {
            assert_eq!(p.status, "succeeded");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let lines = std::fs::read_to_string(root.0.join("runs").join(format!("{id}.ndjson"))).unwrap();
    let actual: Vec<Value> = lines
        .lines()
        .map(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            let mut result = Map::new();
            for key in ["type", "nodeId", "gateId", "slotId", "attempt"] {
                if let Some(value) = event.get(key) {
                    result.insert(key.into(), value.clone());
                }
            }
            for key in ["status", "verdict", "outcome", "code"] {
                if let Some(value) = event["data"].get(key) {
                    result.insert(key.into(), value.clone());
                }
            }
            Value::Object(result)
        })
        .collect();
    let expected: Vec<Value> = include_str!("parity/runs/archon-ledgers/solo-lab.ndjson")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(actual, expected);
    let request = RunStart {
        mission: "admission-findings".into(),
        input_card_id: "inp".into(),
        expected_rev: 1,
        inputs: BTreeMap::from([
            ("unexpected".into(), "x".into()),
            ("sketch".into(), "relative".into()),
        ]),
        ..Default::default()
    };
    let Err(AdmissionFailure::Findings(findings)) = coordinator.start_mission(request) else {
        panic!("admission did not reject")
    };
    let actual = serde_json::to_value(findings).unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "parity/runs/archon-ledgers/admission-findings.json"
    ))
    .unwrap();
    assert_eq!(actual, expected);
}
