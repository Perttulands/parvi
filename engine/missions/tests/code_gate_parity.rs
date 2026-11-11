#[allow(dead_code)]
mod run_support;
use parvi_mission_model::RunStart;
use parvi_missions::{Coordinator, LabExecutor, RunSettings};
use run_support::{Scratch, gated_mission, normalized_ledger};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[test]
fn code_check_contains_matches_archon_ledger() {
    let root = Scratch::new();
    let mission = gated_mission(
        "kinds = [\"code\"]\ncheck = \"output_contains\"\ncheckVersion = \"1\"\ncheckValue = \"hello\"",
    );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "hello".into())]),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" {
            assert_eq!(p.status, "succeeded");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let expected: Vec<Value> =
        include_str!("parity/runs/archon-ledgers/code-check-contains.ndjson")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(normalized_ledger(&root, &id), expected);
    run_support::assert_payload_parity(
        &root,
        &id,
        include_str!("parity/runs/archon-ledgers/code-check-contains.payload.ndjson"),
    );
}

#[test]
fn a_tampered_code_result_cannot_accept_a_human_verdict() {
    let root = Scratch::new();
    let mission = gated_mission(
        "kinds = [\"code\", \"human\"]\ncheck = \"output_contains\"\ncheckVersion = \"1\"\ncheckValue = \"hello\"",
    );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "hello".into())]),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let seq = loop {
        let p = c.run(&id).unwrap();
        if let Some(gate) = p.waiting_gates.first() {
            break gate.requested_seq;
        }
        assert!(p.status != "blocked" && Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    let path = root.0.join("runs").join(format!("{id}.ndjson"));
    let original = std::fs::read_to_string(&path).unwrap();
    let altered = original
        .lines()
        .map(|line| {
            let mut event: Value = serde_json::from_str(line).unwrap();
            if event["type"] == "gate_kind_result" {
                event["data"]["reason"] = "forged".into();
            }
            serde_json::to_string(&event).unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&path, altered).unwrap();
    let result = c.record_verdict(
        &id,
        "gate_review",
        parvi_mission_model::HumanVerdict {
            requested_seq: seq,
            verdict: "pass".into(),
            ..Default::default()
        },
    );
    assert!(result.is_err());
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("human_verdict_recorded")
    );
    std::fs::write(&path, original).unwrap();
    c.abort(&id, "human:operator", "test complete").unwrap();
}

#[test]
fn mixed_code_and_human_gate_preserves_its_durable_code_result() {
    let root = Scratch::new();
    let mission = gated_mission(
        "kinds = [\"code\", \"human\"]\ncheck = \"output_contains\"\ncheckVersion = \"1\"\ncheckValue = \"hello\"",
    );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "hello".into())]),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let requested = loop {
        let p = c.run(&id).unwrap();
        if let Some(gate) = p.waiting_gates.first() {
            break gate.requested_seq;
        }
        assert!(p.status != "blocked" && Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    c.record_verdict(
        &id,
        "gate_review",
        parvi_mission_model::HumanVerdict {
            requested_seq: requested,
            verdict: "pass".into(),
            response: "approved".into(),
            ..Default::default()
        },
    )
    .unwrap();
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" {
            assert_eq!(p.status, "succeeded");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let events: Vec<Value> =
        std::fs::read_to_string(root.0.join("runs").join(format!("{id}.ndjson")))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    let code = events
        .iter()
        .find(|event| event["type"] == "gate_kind_result")
        .unwrap();
    let verdict = events
        .iter()
        .find(|event| event["type"] == "gate_verdict")
        .unwrap();
    assert_eq!(verdict["data"]["kindResultSeqs"]["code"], code["seq"]);
    assert_eq!(verdict["data"]["codeVerdict"], "pass");
    assert_eq!(verdict["data"]["perKind"]["human"], "pass");
    assert_eq!(verdict["data"]["evidence"], code["data"]["evidence"]);
}

#[test]
fn starved_join_blocks_without_allowing_resume() {
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let join = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_join")
        .replace("slot_work", "slot_join")
        .replace(
            "[[formation.output]]",
            "[[formation.input]]\nid = \"port_second\"\nlabel = \"Second\"\n[[formation.output]]",
        );
    let mission = gated_mission("kinds = [\"code\"]\ncheck = \"output_contains\"\ncheckVersion = \"1\"\ncheckValue = \"hello\"").replace("to = \"end_rejected:in\"", "to = \"fmn_join:port_second\"");
    let mission = format!(
        "{mission}\n[[formation]]{join}\n[[connection]]\nid = \"edge_join_input\"\nfrom = \"fmn_work:port_out\"\nto = \"fmn_join:port_in\"\n[[connection]]\nid = \"edge_join_done\"\nfrom = \"fmn_join:port_out\"\nto = \"end_done:in\"\n"
    );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "hello".into())]),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" {
            assert_eq!(p.status, "blocked");
            assert_eq!(p.block_code, "reachable_node_starved");
            assert!(!p.resume_allowed);
            assert!(
                !p.events
                    .iter()
                    .any(|e| e.kind == "node_started" && e.node_id == "fmn_join")
            );
            let expected: Vec<Value> =
                include_str!("parity/runs/archon-ledgers/starved-join.ndjson")
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            assert_eq!(normalized_ledger(&root, &id), expected);
            run_support::assert_payload_parity(
                &root,
                &id,
                include_str!("parity/runs/archon-ledgers/starved-join.payload.ndjson"),
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
