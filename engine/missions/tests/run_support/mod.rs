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
    payload_parity(root, id, expected, false);
}

/// Concurrent branches have independent event order; preserve each node's full
/// sequence and verify the recorded verdict references the actual request.
pub fn assert_concurrent_payload_parity(root: &Scratch, id: &str, expected: &str) {
    payload_parity(root, id, expected, true);
}

pub fn assert_concurrent_ledger(actual: &[Value], expected: &[Value]) {
    fn streams(events: &[Value]) -> std::collections::BTreeMap<String, Vec<Value>> {
        let mut result = std::collections::BTreeMap::<String, Vec<Value>>::new();
        for event in events {
            let key = event["nodeId"].as_str().unwrap_or("run").to_owned();
            result.entry(key).or_default().push(event.clone());
        }
        result
    }
    assert_eq!(streams(actual), streams(expected));
}

fn payload_parity(root: &Scratch, id: &str, expected: &str, concurrent: bool) {
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
    let request_seq = if concurrent {
        let event = text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|event| event["type"] == "human_input_requested");
        event.map(|event| event["seq"].as_u64().unwrap())
    } else {
        None
    };
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
                    if concurrent && key == "requestedSeq" {
                        assert_eq!(
                            value.as_u64(),
                            request_seq,
                            "verdict must reference its actual request"
                        );
                        value = Value::String("<request>".into());
                    }
                    stable(&mut value, id);
                    result.insert(key.into(), value);
                }
            }
            Some(Value::Object(result))
        })
        .collect();
    let mut expected: Vec<Value> = expected
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    if concurrent {
        for event in &mut expected {
            if let Some(value) = event.get_mut("requestedSeq") {
                *value = Value::String("<request>".into());
            }
        }
        assert_concurrent_ledger(&actual, &expected);
    } else {
        assert_eq!(actual, expected);
    }
}
