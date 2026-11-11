use serde_json::{Map, Value};
use std::path::PathBuf;

pub struct Scratch(pub PathBuf);
impl Scratch {
    pub fn new() -> Self {
        let mut b = [0; 8];
        getrandom::fill(&mut b).unwrap();
        let p = std::env::temp_dir().join(format!("parvi-99-run-parity-{}", u64::from_ne_bytes(b)));
        std::fs::create_dir_all(p.join("missions")).unwrap();
        std::fs::create_dir_all(p.join("roles")).unwrap();
        std::fs::write(
            p.join("roles/critic.toml"),
            include_str!("../parity/roles/critic.toml"),
        )
        .unwrap();
        std::fs::write(
            p.join("missions/hello.mission.toml"),
            include_str!("../parity/runs/missions/solo-lab.mission.toml"),
        )
        .unwrap();
        std::fs::write(
            p.join("missions/admission-findings.mission.toml"),
            include_str!("../parity/runs/missions/admission-findings.mission.toml"),
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
pub fn normalized_ledger(root: &Scratch, id: &str) -> Vec<Value> {
    let lines = std::fs::read_to_string(root.0.join("runs").join(format!("{id}.ndjson"))).unwrap();
    lines
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
        .collect()
}

pub fn gated_mission(settings: &str) -> String {
    let solo = include_str!("../parity/runs/missions/solo-lab.mission.toml");
    let mission = solo.replace("to = \"end_done:in\"", "to = \"gate_review:in\"");
    format!(
        "{mission}\n[[gate]]\nid = \"gate_review\"\ntitle = \"Review\"\n{settings}\n\n[[end]]\nid = \"end_rejected\"\ntitle = \"Rejected\"\noutcome = \"rejected\"\n\n[[connection]]\nid = \"edge_pass\"\nfrom = \"gate_review:pass\"\nto = \"end_done:in\"\n\n[[connection]]\nid = \"edge_fail\"\nfrom = \"gate_review:fail\"\nto = \"end_rejected:in\"\n"
    )
}

pub fn assert_payload_parity(root: &Scratch, id: &str, expected: &str) {
    fn stable(value: &mut Value, id: &str) {
        match value {
            Value::String(text) => {
                *text = text
                    .replace(&format!("ledger://{id}/"), "ledger://<run>/")
                    .replace("archon-verdict", "parvi-verdict")
                    .replace("archon-outputs", "parvi-outputs")
            }
            Value::Array(values) => values.iter_mut().for_each(|v| stable(v, id)),
            Value::Object(values) => {
                for key in ["artifactRef", "reportRef", "kind"] {
                    if values.get(key).and_then(Value::as_str) == Some("") {
                        values.remove(key);
                    }
                }
                values.values_mut().for_each(|v| stable(v, id));
            }
            _ => {}
        }
    }
    let text = std::fs::read_to_string(root.0.join("runs").join(format!("{id}.ndjson"))).unwrap();
    let actual: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter_map(|event| {
            let kind = event["type"].as_str().unwrap();
            if ![
                "node_started",
                "gate_evaluating",
                "gate_kind_result",
                "human_verdict_recorded",
                "gate_verdict",
                "run_failed",
            ]
            .contains(&kind)
            {
                return None;
            }
            let mut result = Map::new();
            for key in ["type", "nodeId", "gateId", "attempt"] {
                if let Some(value) = event.get(key) {
                    result.insert(key.into(), value.clone());
                }
            }
            for key in [
                "inputRefs",
                "inputRef",
                "reason",
                "requestedSeq",
                "routedEdges",
                "verdict",
                "evidence",
            ] {
                if let Some(value) = event["data"].get(key) {
                    let mut value = value.clone();
                    stable(&mut value, id);
                    result.insert(key.into(), value);
                }
            }
            Some(Value::Object(result))
        })
        .collect();
    let expected: Vec<Value> = expected
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(actual, expected);
}
