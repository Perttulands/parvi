#[allow(dead_code)]
mod run_support;
use parvi_missions::{Coordinator, LabExecutor, RunSettings, RunStart};
use run_support::{Scratch, gated_mission, normalized_ledger};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[test]
fn lab_judge_routes_a_synthetic_verdict() {
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let judge = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_judge")
        .replace("slot_work", "slot_judge");
    let mission = solo.replace("to = \"end_done:in\"", "to = \"gate_review:in\"");
    let mission = format!(
        "{mission}\n[[formation]]{judge}\n[[gate]]\nid = \"gate_review\"\ntitle = \"Review\"\nkinds = [\"formation\"]\n\n[[end]]\nid = \"end_rejected\"\ntitle = \"Rejected\"\noutcome = \"rejected\"\n\n[[connection]]\nid = \"edge_pass\"\nfrom = \"gate_review:pass\"\nto = \"end_done:in\"\n\n[[connection]]\nid = \"edge_fail\"\nfrom = \"gate_review:fail\"\nto = \"end_rejected:in\"\n\n[[connection]]\nid = \"edge_judge\"\nfrom = \"gate_review:judge\"\nto = \"fmn_judge:port_in\"\n\n[[connection]]\nid = \"edge_return\"\nfrom = \"fmn_judge:port_out\"\nto = \"gate_review:judge\"\n"
    );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([(
                "brief".into(),
                "\n```parvi-verdict\n{\"verdict\":\"pass\",\"reason\":\"ok\",\"evidence\":[]}\n```"
                    .into(),
            )]),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" {
            assert_eq!(
                p.status,
                "succeeded",
                "{}",
                std::fs::read_to_string(root.0.join("runs").join(format!("{id}.ndjson"))).unwrap()
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let expected: Vec<Value> = include_str!("parity/runs/archon-ledgers/judge-pass.ndjson")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(normalized_ledger(&root, &id), expected);
    run_support::assert_payload_parity(
        &root,
        &id,
        include_str!("parity/runs/archon-ledgers/judge-pass.payload.ndjson"),
    );
    // A start that names no actor is Parvi's own (#174).
    assert_eq!(c.run(&id).unwrap().started_by, "agent:parvi");
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "no verdict".into())]),
            ..Default::default()
        })
        .unwrap();
    loop {
        let p = c.run(&id).unwrap();
        if p.status == "blocked" {
            assert_eq!(p.block_code, "invalid_judge_result");
            assert!(p.resume_allowed);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    loop {
        match c.resume(
            &id,
            parvi_missions::ResumeRequest {
                take_fail: true,
                actor: "human:operator".into(),
                ..Default::default()
            },
        ) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("resume: {e}"),
        }
    }
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" {
            assert_eq!(p.status, "failed");
            assert_eq!(p.events.last().unwrap().code, "path_rejected");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let expected: Vec<Value> =
        include_str!("parity/runs/archon-ledgers/judge-malformed-then-take-fail.ndjson")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(normalized_ledger(&root, &id), expected);
    run_support::assert_payload_parity(
        &root,
        &id,
        include_str!("parity/runs/archon-ledgers/judge-malformed-then-take-fail.payload.ndjson"),
    );
}

struct ScriptedLab(std::sync::Arc<std::sync::Mutex<Vec<(String, i64, String)>>>);
impl parvi_missions::Executor for ScriptedLab {
    fn execute_formation(
        &self,
        request: parvi_missions::FormationExecution,
        events: parvi_missions::ExecutionSink,
    ) -> Result<parvi_missions::FormationResult, String> {
        let node = request.formation.id.clone();
        let attempt = request.attempt;
        self.0
            .lock()
            .unwrap()
            .push((node.clone(), attempt, request.input_text.clone()));
        let mut result =
            parvi_missions::Executor::execute_formation(&LabExecutor, request, events)?;
        let text = if node == "fmn_judge2" {
            if attempt == 1 {
                "```parvi-verdict\n{invalid}\n```"
            } else {
                "```parvi-verdict\n{\"verdict\":\"pass\",\"reason\":\"fixed\",\"evidence\":[]}\n```"
            }
        } else if node == "fmn_judge1" {
            "earlier review"
        } else if node == "fmn_judge" {
            if attempt == 1 {
                "```parvi-verdict\n{\"verdict\":\"fail\",\"reason\":\"add coverage\",\"evidence\":[\"missing test\"]}\n```"
            } else {
                "```parvi-verdict\n{\"verdict\":\"pass\",\"reason\":\"covered\",\"evidence\":[]}\n```"
            }
        } else if attempt == 1 {
            "draft one"
        } else {
            "draft two"
        };
        result.text = text.into();
        for output in result.outputs.values_mut() {
            output["text"] = text.into();
        }
        Ok(result)
    }
}

struct CorrectingLab(std::sync::Arc<std::sync::Mutex<Vec<(String, i64)>>>);
impl parvi_missions::Executor for CorrectingLab {
    fn execute_formation(
        &self,
        request: parvi_missions::FormationExecution,
        events: parvi_missions::ExecutionSink,
    ) -> Result<parvi_missions::FormationResult, String> {
        parvi_missions::Executor::execute_formation(&LabExecutor, request, events)
    }
    fn correct_judge(
        &self,
        request: parvi_missions::JudgeCorrection,
        _: parvi_missions::ExecutionSink,
    ) -> Result<Option<String>, String> {
        self.0
            .lock()
            .unwrap()
            .push((request.node_id, request.attempt));
        assert!(request.error.contains("missing"));
        Ok(Some("```parvi-verdict\n{\"verdict\":\"pass\",\"reason\":\"corrected\",\"evidence\":[]}\n```\n```parvi-outputs\n{\"port_out\":{\"text\":\"corrected\"}}\n```".into()))
    }
}

#[test]
fn judge_gets_one_correction_on_its_existing_attempt() {
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let judge = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_judge")
        .replace("slot_work", "slot_judge");
    let mission = format!(
        "{}\n[[formation]]{judge}\n[[connection]]\nid = \"edge_judge\"\nfrom = \"gate_review:judge\"\nto = \"fmn_judge:port_in\"\n[[connection]]\nid = \"edge_return\"\nfrom = \"fmn_judge:port_out\"\nto = \"gate_review:judge\"\n",
        gated_mission("kinds = [\"formation\"]")
    );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let corrections = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let c = Coordinator::open(
        &root.0,
        RunSettings::default(),
        Box::new(CorrectingLab(corrections.clone())),
    )
    .unwrap();
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
            assert_eq!(
                p.events
                    .iter()
                    .filter(|event| event.kind == "node_started" && event.node_id == "fmn_judge")
                    .count(),
                1
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(*corrections.lock().unwrap(), vec![("fmn_judge".into(), 1)]);
}

#[test]
fn resuming_invalid_final_judge_keeps_the_earlier_review() {
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let judge = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap();
    let j1 = judge
        .replace("fmn_work", "fmn_judge1")
        .replace("slot_work", "slot_judge1");
    let j2 = judge
        .replace("fmn_work", "fmn_judge2")
        .replace("slot_work", "slot_judge2");
    let mission = format!(
        "{}\n[[formation]]{j1}\n[[formation]]{j2}\n[[connection]]\nid = \"edge_judge\"\nfrom = \"gate_review:judge\"\nto = \"fmn_judge1:port_in\"\n[[connection]]\nid = \"edge_chain\"\nfrom = \"fmn_judge1:port_out\"\nto = \"fmn_judge2:port_in\"\n[[connection]]\nid = \"edge_return\"\nfrom = \"fmn_judge2:port_out\"\nto = \"gate_review:judge\"\n",
        gated_mission("kinds = [\"formation\"]")
    );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let c = Coordinator::open(
        &root.0,
        RunSettings::default(),
        Box::new(ScriptedLab(calls.clone())),
    )
    .unwrap();
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
        if p.status == "blocked" {
            assert_eq!(p.block_code, "invalid_judge_result");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    loop {
        match c.resume(&id, Default::default()) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("resume: {e}"),
        }
    }
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" {
            assert_eq!(p.status, "succeeded");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|(node, _, _)| node == "fmn_judge1")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(node, _, _)| node == "fmn_judge2")
            .map(|(_, attempt, _)| *attempt)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn judge_fail_pushes_back_typed_feedback_before_a_later_pass() {
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let judge = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_judge")
        .replace("slot_work", "slot_judge");
    let mission = gated_mission("kinds = [\"formation\"]")
        .replace("to = \"end_rejected:in\"", "to = \"fmn_work:port_in\"");
    let mission = format!(
        "{mission}\n[[formation]]{judge}\n[[connection]]\nid = \"edge_judge\"\nfrom = \"gate_review:judge\"\nto = \"fmn_judge:port_in\"\n[[connection]]\nid = \"edge_return\"\nfrom = \"fmn_judge:port_out\"\nto = \"gate_review:judge\"\n"
    );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let c = Coordinator::open(
        &root.0,
        RunSettings::default(),
        Box::new(ScriptedLab(calls.clone())),
    )
    .unwrap();
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
    let calls = calls.lock().unwrap();
    let retry = &calls
        .iter()
        .find(|(node, attempt, _)| node == "fmn_work" && *attempt == 2)
        .unwrap()
        .2;
    for expected in [
        "gate feedback from gate_review, attempt 1:",
        "reason: add coverage",
        "evidence: missing test",
        "original input: draft one",
    ] {
        assert!(
            retry.contains(expected),
            "missing {expected:?} in {retry:?}"
        );
    }
    let expected: Vec<Value> =
        include_str!("parity/runs/archon-ledgers/judge-fail-pushback.ndjson")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(normalized_ledger(&root, &id), expected);
    run_support::assert_payload_parity(
        &root,
        &id,
        include_str!("parity/runs/archon-ledgers/judge-fail-pushback.payload.ndjson"),
    );
}

#[test]
fn delivery_example_reaches_its_end_with_a_simulated_judge() {
    let root = Scratch::new();
    std::fs::write(
        root.0.join("missions/delivery.mission.toml"),
        include_str!("fixtures/delivery.mission.toml"),
    )
    .unwrap();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c.start_mission(RunStart { mission: "delivery".into(), input_card_id: "mis_delivery".into(), expected_rev: 1, inputs: BTreeMap::from([("change".into(), "simulated\n```parvi-verdict\n{\"verdict\":\"pass\",\"reason\":\"simulated review\",\"evidence\":[]}\n```".into())]), ..Default::default() }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" {
            assert_eq!(
                p.status,
                "succeeded",
                "{}",
                std::fs::read_to_string(root.0.join("runs").join(format!("{id}.ndjson"))).unwrap()
            );
            assert!(
                p.events
                    .last()
                    .unwrap()
                    .end_ids
                    .contains(&"end_delivered".into())
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn concurrent_judge_and_step_publish_only_their_own_files() {
    use parvi_missions::{ExecutionSink, Executor, FormationExecution, FormationResult};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Writing(Arc<AtomicUsize>);
    impl Executor for Writing {
        fn execute_formation(
            &self,
            req: FormationExecution,
            sink: ExecutionSink,
        ) -> Result<FormationResult, String> {
            let node = req.formation.id.clone();
            if node == "fmn_other" || node == "fmn_judge" {
                std::fs::write(req.cwd.join(&node), format!("{node} partial"))
                    .map_err(|e| e.to_string())?;
                self.0.fetch_add(1, Ordering::SeqCst);
                let deadline = Instant::now() + Duration::from_secs(5);
                while self.0.load(Ordering::SeqCst) < 2 {
                    if Instant::now() > deadline {
                        return Err("parallel turns did not overlap".into());
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                if node == "fmn_other" {
                    std::thread::sleep(Duration::from_millis(300));
                }
                std::fs::write(req.cwd.join(&node), format!("{node} final"))
                    .map_err(|e| e.to_string())?;
            }
            LabExecutor.execute_formation(req, sink)
        }
    }
    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    let root = Scratch::new();
    let checkout = root.0.join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    git(&checkout, &["init", "-b", "main"]);
    git(&checkout, &["config", "user.email", "test@example.invalid"]);
    git(&checkout, &["config", "user.name", "Test"]);
    std::fs::write(checkout.join("base"), "base").unwrap();
    git(&checkout, &["add", "base"]);
    git(&checkout, &["commit", "-m", "base"]);
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let formation = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap();
    let judge = formation
        .replace("fmn_work", "fmn_judge")
        .replace("slot_work", "slot_judge");
    let other = formation
        .replace("fmn_work", "fmn_other")
        .replace("slot_work", "slot_other");
    let mission = gated_mission("kinds = [\"formation\"]")
        + &format!(
            "\n[[formation]]{judge}\n[[formation]]{other}\n[[connection]]\nid = \"edge_judge\"\nfrom = \"gate_review:judge\"\nto = \"fmn_judge:port_in\"\n[[connection]]\nid = \"edge_return\"\nfrom = \"fmn_judge:port_out\"\nto = \"gate_review:judge\"\n[[connection]]\nid = \"edge_other\"\nfrom = \"inp_hello:out\"\nto = \"fmn_other:port_in\"\n[[connection]]\nid = \"edge_other_end\"\nfrom = \"fmn_other:port_out\"\nto = \"end_done:in\"\n"
        );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let settings = RunSettings {
        repositories: BTreeMap::from([("owner/repo".into(), checkout)]),
        ..Default::default()
    };
    let c = Coordinator::open(
        &root.0,
        settings,
        Box::new(Writing(Arc::new(AtomicUsize::new(0)))),
    )
    .unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            repository: "owner/repo".into(),
            inputs: BTreeMap::from([(
                "brief".into(),
                "```parvi-verdict\n{\"verdict\":\"pass\",\"reason\":\"ok\",\"evidence\":[]}\n```"
                    .into(),
            )]),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let p = loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" {
            break p;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(p.status, "succeeded", "{:?}", p.events.last());
    let workspace = root.0.join("worktrees").join(&id);
    let events = parvi_missions::Ledger::new(root.0.join("runs").join(format!("{id}.ndjson")), &id)
        .read()
        .unwrap();
    for node in ["fmn_judge", "fmn_other"] {
        let commit = events
            .iter()
            .find(|e| e.kind == "workspace_commit" && e.node_id == node)
            .unwrap()
            .data["commit"]
            .as_str()
            .unwrap();
        assert_eq!(
            git(&workspace, &["show", "--format=", "--name-only", commit]),
            node
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join(node)).unwrap(),
            format!("{node} final")
        );
    }
}
