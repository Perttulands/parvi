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
fn reattach_keeps_other_completed_dispatches_when_one_fails() {
    use parvi_missions::{
        ExecutionEvent, ExecutionSink, Executor, FormationExecution, FormationResult, OpenDispatch,
        ResumeMode, ResumeRequest,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Seats {
        release: Arc<AtomicBool>,
        fail: bool,
    }
    impl Executor for Seats {
        fn execute_formation(
            &self,
            req: FormationExecution,
            sink: ExecutionSink,
        ) -> Result<FormationResult, String> {
            let slot = &req.formation.slots[0];
            let dispatch = format!("dsp_{}_{}", req.formation.id, req.attempt);
            sink.emit(ExecutionEvent { kind:"slot_dispatch".into(), slot_id:slot.id.clone(), data:serde_json::json!({"dispatchId":dispatch,"nodeId":req.formation.id,"slotId":slot.id}).as_object().unwrap().clone() })?;
            while !self.release.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(5));
            }
            sink.emit(ExecutionEvent { kind:"slot_result".into(), slot_id:slot.id.clone(), data:serde_json::json!({"dispatchId":dispatch,"status":"ok","sentinel":{"runId":req.run_id}}).as_object().unwrap().clone() })?;
            Ok(FormationResult {
                status: "done".into(),
                text: "done".into(),
                outputs: req
                    .formation
                    .outputs
                    .iter()
                    .map(|p| (p.id.clone(), serde_json::json!({"text":"done"})))
                    .collect(),
            })
        }
        fn reattach(&self, req: OpenDispatch) -> Result<FormationResult, String> {
            if self.fail && req.node_id == "fmn_other" {
                return Err("capture unavailable".into());
            }
            Ok(FormationResult {
                status: "done".into(),
                text: "done".into(),
                outputs: req
                    .formation
                    .outputs
                    .iter()
                    .map(|p| (p.id.clone(), serde_json::json!({"text":"done"})))
                    .collect(),
            })
        }
    }
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let other = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_other")
        .replace("slot_work", "slot_other");
    let mission = format!(
        "{solo}\n[[formation]]{other}\n[[connection]]\nid = \"edge_other\"\nfrom = \"inp_hello:out\"\nto = \"fmn_other:port_in\"\n[[connection]]\nid = \"edge_other_end\"\nfrom = \"fmn_other:port_out\"\nto = \"end_done:in\"\n"
    );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let release = Arc::new(AtomicBool::new(false));
    let original = Coordinator::open(
        &root.0,
        RunSettings::default(),
        Box::new(Seats {
            release: release.clone(),
            fail: false,
        }),
    )
    .unwrap();
    let id = original
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "hello".into())]),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if original
            .run(&id)
            .unwrap()
            .events
            .iter()
            .filter(|e| e.kind == "slot_dispatch")
            .count()
            == 2
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let scratch = Scratch::new();
    let source = root.0.join("runs");
    let target = scratch.0.join("runs");
    std::fs::create_dir_all(&target).unwrap();
    for suffix in ["ndjson", "snapshot.toml", "bindings.toml"] {
        std::fs::copy(
            source.join(format!("{id}.{suffix}")),
            target.join(format!("{id}.{suffix}")),
        )
        .unwrap();
    }
    release.store(true, Ordering::Release);
    while !original.run(&id).unwrap().r#final {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(original);
    let c = Coordinator::open(
        &scratch.0,
        RunSettings::default(),
        Box::new(Seats {
            release,
            fail: true,
        }),
    )
    .unwrap();
    loop {
        let p = c.run(&id).unwrap();
        if p.block_code == "dispatch_reattach_failed" {
            assert!(
                p.events
                    .iter()
                    .any(|e| e.kind == "node_output" && e.node_id == "fmn_work")
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let events = parvi_missions::Ledger::new(target.join(format!("{id}.ndjson")), &id)
        .read()
        .unwrap();
    let block = events
        .iter()
        .rev()
        .find(|e| e.kind == "run_blocked")
        .unwrap();
    let remaining = block.data["openDispatches"].as_array().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0]["nodeId"], "fmn_other");
    loop {
        match c.resume(
            &id,
            ResumeRequest {
                mode: ResumeMode::Redispatch,
                ..Default::default()
            },
        ) {
            Ok(_) => break,
            Err(e)
                if e.kind() == std::io::ErrorKind::AlreadyExists && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("resume: {e}"),
        }
    }
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final {
            assert_eq!(p.status, "succeeded");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn restart_cuts_preserve_branch_and_human_gate_outcomes() {
    struct JudgeLab;
    impl parvi_missions::Executor for JudgeLab {
        fn execute_formation(
            &self,
            request: parvi_missions::FormationExecution,
            sink: parvi_missions::ExecutionSink,
        ) -> Result<parvi_missions::FormationResult, String> {
            let judge = request.formation.id == "fmn_judge";
            let pass = request.input_text.contains("needs work");
            let mut result =
                parvi_missions::Executor::execute_formation(&LabExecutor, request, sink)?;
            if judge {
                result.text = if pass { "```parvi-verdict\n{\"verdict\":\"pass\",\"reason\":\"fixed\",\"evidence\":[]}\n```" } else { "```parvi-verdict\n{\"verdict\":\"fail\",\"reason\":\"needs work\",\"evidence\":[]}\n```" }.into();
                for output in result.outputs.values_mut() {
                    output["text"] = result.text.clone().into();
                }
            }
            Ok(result)
        }
    }
    fn drive(c: &Coordinator, id: &str, retry: bool) -> parvi_missions::RunProjection {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let p = c.run(id).unwrap();
            if p.r#final {
                return p;
            }
            if p.status == "blocked" {
                assert!(p.resume_allowed, "{}", p.block_code);
                match c.resume(
                    id,
                    parvi_missions::ResumeRequest {
                        mode: parvi_missions::ResumeMode::Redispatch,
                        ..Default::default()
                    },
                ) {
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => panic!("resume: {e}"),
                }
            } else if let Some(gate) = p.waiting_gates.first() {
                match c.record_verdict(
                    id,
                    &gate.gate_id,
                    parvi_missions::HumanVerdict {
                        requested_seq: gate.requested_seq,
                        verdict: if retry
                            && !p.events.iter().any(|event| {
                                event.kind == "human_verdict_recorded"
                                    && event.gate_id == gate.gate_id
                            }) {
                            "fail"
                        } else {
                            "pass"
                        }
                        .into(),
                        ..Default::default()
                    },
                ) {
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => panic!("verdict: {e}"),
                }
            }
            assert!(Instant::now() < deadline, "run {id} did not settle");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn outcome(
        p: &parvi_missions::RunProjection,
    ) -> (String, Vec<String>, BTreeMap<String, usize>) {
        let mut counts = BTreeMap::new();
        for event in &p.events {
            if event.kind == "node_output" {
                *counts.entry(event.node_id.clone()).or_default() += 1;
            }
        }
        (
            p.status.clone(),
            p.events.last().unwrap().end_ids.clone(),
            counts,
        )
    }
    for scenario in 0..3 {
        let retry = scenario == 1;
        let root = Scratch::new();
        let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
        let other = solo
            .split("[[formation]]")
            .nth(1)
            .unwrap()
            .split("[[end]]")
            .next()
            .unwrap()
            .replace("fmn_work", "fmn_other")
            .replace("slot_work", "slot_other")
            .replace(
                "Return a short result",
                "archon-lab-work:40ms Return a short result",
            );
        let mut mission = format!(
            "{}\n[[formation]]{other}\n[[connection]]\nid = \"edge_other\"\nfrom = \"inp_hello:out\"\nto = \"fmn_other:port_in\"\n[[connection]]\nid = \"edge_other_end\"\nfrom = \"fmn_other:port_out\"\nto = \"end_done:in\"\n",
            gated_mission("kinds = [\"human\"]")
        );
        if retry || scenario == 2 {
            mission = mission.replace("to = \"end_rejected:in\"", "to = \"fmn_work:port_in\"");
        }
        if scenario == 2 {
            mission = mission.replace("kinds = [\"human\"]", "kinds = [\"formation\"]");
            let judge = solo
                .split("[[formation]]")
                .nth(1)
                .unwrap()
                .split("[[end]]")
                .next()
                .unwrap()
                .replace("fmn_work", "fmn_judge")
                .replace("slot_work", "slot_judge");
            mission.push_str(&format!("\n[[formation]]{judge}\n[[connection]]\nid = \"edge_judge\"\nfrom = \"gate_review:judge\"\nto = \"fmn_judge:port_in\"\n[[connection]]\nid = \"edge_judge_return\"\nfrom = \"fmn_judge:port_out\"\nto = \"gate_review:judge\"\n"));
        }
        std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
        let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(JudgeLab)).unwrap();
        let id = c
            .start_mission(RunStart {
                mission: "hello".into(),
                input_card_id: "inp_hello".into(),
                expected_rev: 1,
                inputs: BTreeMap::from([("brief".into(), "hello".into())]),
                ..Default::default()
            })
            .unwrap();
        let expected = outcome(&drive(&c, &id, retry));
        drop(c);
        let runs = root.0.join("runs");
        let ledger = std::fs::read_to_string(runs.join(format!("{id}.ndjson"))).unwrap();
        let lines: Vec<_> = ledger.lines().collect();
        for cut in 1..lines.len() {
            let scratch = Scratch::new();
            let target = scratch.0.join("runs");
            std::fs::create_dir_all(&target).unwrap();
            for suffix in ["snapshot.toml", "bindings.toml"] {
                std::fs::copy(
                    runs.join(format!("{id}.{suffix}")),
                    target.join(format!("{id}.{suffix}")),
                )
                .unwrap();
            }
            std::fs::write(
                target.join(format!("{id}.ndjson")),
                lines[..cut].join("\n") + "\n",
            )
            .unwrap();
            let c =
                Coordinator::open(&scratch.0, RunSettings::default(), Box::new(JudgeLab)).unwrap();
            assert_eq!(
                outcome(&drive(&c, &id, retry)),
                expected,
                "scenario {scenario}, cut {cut}"
            );
        }
    }
}

#[test]
fn fan_out_runs_concurrently_and_join_waits_for_both() {
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let formation = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap();
    let first = formation
        .replace("fmn_work", "fmn_a")
        .replace("slot_work", "slot_a")
        .replace(
            "Return a short result",
            "archon-lab-work:200ms Return a short result",
        );
    let second = first
        .replace("fmn_a", "fmn_b")
        .replace("slot_a", "slot_b")
        .replace("200ms", "500ms");
    let join = formation
        .replace("fmn_work", "fmn_join")
        .replace("slot_work", "slot_join")
        .replace(
            "[[formation.output]]",
            "[[formation.input]]\nid = \"port_second\"\nlabel = \"Second\"\n[[formation.output]]",
        );
    let prefix = solo.split("[[formation]]").next().unwrap();
    let end = solo
        .split("[[end]]")
        .nth(1)
        .unwrap()
        .split("[[connection]]")
        .next()
        .unwrap();
    let edges = [
        ("inp_hello:out", "fmn_a:port_in"),
        ("inp_hello:out", "fmn_b:port_in"),
        ("fmn_a:port_out", "fmn_join:port_in"),
        ("fmn_b:port_out", "fmn_join:port_second"),
        ("fmn_join:port_out", "end_done:in"),
    ];
    let mut mission =
        format!("{prefix}[[formation]]{first}[[formation]]{second}[[formation]]{join}[[end]]{end}");
    for (i, (from, to)) in edges.iter().enumerate() {
        mission.push_str(&format!(
            "\n[[connection]]\nid = \"edge_{i}\"\nfrom = \"{from}\"\nto = \"{to}\"\n"
        ));
    }
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let start = Instant::now();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "hello".into())]),
            ..Default::default()
        })
        .unwrap();
    let deadline = start + Duration::from_secs(10);
    let projection = loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" {
            break p;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(projection.status, "succeeded");
    let events = &projection.events;
    let seq = |kind: &str, node: &str| {
        events
            .iter()
            .find(|e| e.kind == kind && e.node_id == node)
            .unwrap()
            .seq
    };
    assert!(seq("node_started", "fmn_a") < seq("node_output", "fmn_b"));
    assert!(seq("node_started", "fmn_b") < seq("node_output", "fmn_a"));
    assert!(
        seq("node_started", "fmn_join")
            > seq("node_output", "fmn_a").max(seq("node_output", "fmn_b"))
    );
    let expected: Vec<Value> = include_str!("parity/runs/archon-ledgers/fan-out-join.ndjson")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(normalized_ledger(&root, &id), expected);
    run_support::assert_payload_parity(
        &root,
        &id,
        include_str!("parity/runs/archon-ledgers/fan-out-join.payload.ndjson"),
    );
}

#[test]
fn human_gate_records_and_routes_the_exact_request() {
    use parvi_missions::HumanVerdict;
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let mission = solo.replace("to = \"end_done:in\"", "to = \"gate_review:in\"");
    let mission = format!(
        "{mission}\n[[gate]]\nid = \"gate_review\"\ntitle = \"Review\"\nkinds = [\"human\"]\n\n[[end]]\nid = \"end_rejected\"\ntitle = \"Rejected\"\noutcome = \"rejected\"\n\n[[connection]]\nid = \"edge_pass\"\nfrom = \"gate_review:pass\"\nto = \"end_done:in\"\n\n[[connection]]\nid = \"edge_fail\"\nfrom = \"gate_review:fail\"\nto = \"end_rejected:in\"\n"
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
    let request = loop {
        let p = c.run(&id).unwrap();
        if let Some(gate) = p.waiting_gates.first() {
            break gate.requested_seq;
        }
        assert!(p.status != "blocked" && Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    let verdict = HumanVerdict {
        requested_seq: request,
        verdict: "pass".into(),
        response: " yes\n".into(),
        ..Default::default()
    };
    c.record_verdict(&id, "gate_review", verdict.clone())
        .unwrap();
    assert!(c.record_verdict(&id, "gate_review", verdict).is_err());
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final {
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
    let actual = normalized_ledger(&root, &id);
    let expected: Vec<Value> = include_str!("parity/runs/archon-ledgers/human-approve.ndjson")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(actual, expected);
    run_support::assert_payload_parity(
        &root,
        &id,
        include_str!("parity/runs/archon-ledgers/human-approve.payload.ndjson"),
    );
}

#[test]
fn human_answer_routes_while_another_branch_runs_and_rejection_waits() {
    use parvi_missions::HumanVerdict;
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let other = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_other")
        .replace("slot_work", "slot_other")
        .replace(
            "Return a short result",
            "archon-lab-work:700ms Return a short result",
        );
    let mission = format!(
        "{}\n[[formation]]{other}\n[[connection]]\nid = \"edge_other\"\nfrom = \"inp_hello:out\"\nto = \"fmn_other:port_in\"\n[[connection]]\nid = \"edge_other_done\"\nfrom = \"fmn_other:port_out\"\nto = \"end_done:in\"\n",
        gated_mission("kinds = [\"human\"]").replace(
            "Return a short result",
            "archon-lab-work:50ms Return a short result"
        )
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
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    c.record_verdict(
        &id,
        "gate_review",
        HumanVerdict {
            requested_seq: seq,
            verdict: "fail".into(),
            response: "needs work".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let projection = loop {
        let p = c.run(&id).unwrap();
        if p.r#final {
            break p;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(projection.status, "failed");
    let seq = |kind: &str, node: &str| {
        projection
            .events
            .iter()
            .find(|e| e.kind == kind && e.node_id == node)
            .unwrap()
            .seq
    };
    assert!(seq("gate_verdict", "gate_review") < seq("node_output", "fmn_other"));
    assert!(seq("run_failed", "end_rejected") > seq("node_output", "fmn_other"));
    let lines = std::fs::read_to_string(root.0.join("runs").join(format!("{id}.ndjson"))).unwrap();
    let last: Value = serde_json::from_str(lines.lines().last().unwrap()).unwrap();
    assert_eq!(last["data"]["code"], "path_rejected");
    assert_eq!(last["data"]["reason"], "needs work");
    assert_eq!(last["actor"], "human:operator");
    assert!(
        last["data"]["endIds"]
            .as_array()
            .unwrap()
            .contains(&"end_done".into())
    );
    assert!(
        last["data"]["endIds"]
            .as_array()
            .unwrap()
            .contains(&"end_rejected".into())
    );
    let expected: Vec<Value> =
        include_str!("parity/runs/archon-ledgers/rejected-end-on-one-branch.ndjson")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(normalized_ledger(&root, &id), expected);
    run_support::assert_payload_parity(
        &root,
        &id,
        include_str!("parity/runs/archon-ledgers/rejected-end-on-one-branch.payload.ndjson"),
    );
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "hello".into())]),
            ..Default::default()
        })
        .unwrap();
    let requested = loop {
        let p = c.run(&id).unwrap();
        if let Some(gate) = p.waiting_gates.first() {
            break gate.requested_seq;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    c.record_verdict(
        &id,
        "gate_review",
        HumanVerdict {
            requested_seq: requested,
            verdict: "pass".into(),
            response: "yes".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let projection = loop {
        let p = c.run(&id).unwrap();
        if p.r#final {
            break p;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(projection.status, "succeeded");
    let decision = projection
        .events
        .iter()
        .find(|e| e.kind == "gate_verdict")
        .unwrap()
        .seq;
    let output = projection
        .events
        .iter()
        .find(|e| e.kind == "node_output" && e.node_id == "fmn_other")
        .unwrap()
        .seq;
    assert!(decision < output);
    let expected: Vec<Value> =
        include_str!("parity/runs/archon-ledgers/human-approve-while-other-step-works.ndjson")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(normalized_ledger(&root, &id), expected);
    run_support::assert_payload_parity(
        &root,
        &id,
        include_str!(
            "parity/runs/archon-ledgers/human-approve-while-other-step-works.payload.ndjson"
        ),
    );
}

#[test]
fn a_newer_human_request_makes_the_old_sequence_stale() {
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let other = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_other")
        .replace("slot_work", "slot_other")
        .replace(
            "Return a short result",
            "archon-lab-work:400ms Return a short result",
        );
    let base = gated_mission("kinds = [\"human\"]").replace(
        "Return a short result",
        "archon-lab-work:50ms Return a short result",
    );
    let mission = format!(
        "{base}\n[[formation]]{other}\n[[gate]]\nid = \"gate_check\"\ntitle = \"Check\"\nkinds = [\"code\"]\ncheck = \"output_contains\"\ncheckVersion = \"1\"\ncheckValue = \"never present\"\n[[connection]]\nid = \"edge_other\"\nfrom = \"inp_hello:out\"\nto = \"fmn_other:port_in\"\n[[connection]]\nid = \"edge_check\"\nfrom = \"fmn_other:port_out\"\nto = \"gate_check:in\"\n[[connection]]\nid = \"edge_check_pass\"\nfrom = \"gate_check:pass\"\nto = \"end_done:in\"\n[[connection]]\nid = \"edge_check_fail\"\nfrom = \"gate_check:fail\"\nto = \"gate_review:in\"\n"
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
    let first = loop {
        let p = c.run(&id).unwrap();
        if let Some(gate) = p.waiting_gates.first() {
            break gate.requested_seq;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    let second = loop {
        let p = c.run(&id).unwrap();
        if let Some(gate) = p.waiting_gates.first()
            && gate.requested_seq != first
        {
            break gate.requested_seq;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    let stale = c
        .record_verdict(
            &id,
            "gate_review",
            parvi_missions::HumanVerdict {
                requested_seq: first,
                verdict: "pass".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(stale.kind(), std::io::ErrorKind::AlreadyExists);
    c.record_verdict(
        &id,
        "gate_review",
        parvi_missions::HumanVerdict {
            requested_seq: second,
            verdict: "pass".into(),
            ..Default::default()
        },
    )
    .unwrap();
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final {
            assert_eq!(p.status, "succeeded");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let expected: Vec<Value> = include_str!("parity/runs/archon-ledgers/human-stale-409.ndjson")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(normalized_ledger(&root, &id), expected);
    run_support::assert_payload_parity(
        &root,
        &id,
        include_str!("parity/runs/archon-ledgers/human-stale-409.payload.ndjson"),
    );
}

#[test]
fn separate_human_gates_can_be_answered_in_either_order() {
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let other = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_other")
        .replace("slot_work", "slot_other");
    let mission = format!(
        "{}\n[[formation]]{other}\n[[gate]]\nid = \"gate_other\"\ntitle = \"Other\"\nkinds = [\"human\"]\n\n[[connection]]\nid = \"edge_other\"\nfrom = \"inp_hello:out\"\nto = \"fmn_other:port_in\"\n[[connection]]\nid = \"edge_other_gate\"\nfrom = \"fmn_other:port_out\"\nto = \"gate_other:in\"\n[[connection]]\nid = \"edge_other_pass\"\nfrom = \"gate_other:pass\"\nto = \"end_done:in\"\n[[connection]]\nid = \"edge_other_fail\"\nfrom = \"gate_other:fail\"\nto = \"end_rejected:in\"\n",
        gated_mission("kinds = [\"human\"]")
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
    let gates = loop {
        let p = c.run(&id).unwrap();
        if p.waiting_gates.len() == 2 {
            break p.waiting_gates;
        }
        assert!(p.status != "blocked" && Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    for gate in gates.iter().rev() {
        c.record_verdict(
            &id,
            &gate.gate_id,
            parvi_missions::HumanVerdict {
                requested_seq: gate.requested_seq,
                verdict: "pass".into(),
                ..Default::default()
            },
        )
        .unwrap();
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
}

#[test]
fn a_blocked_run_accepts_another_gates_verdict_and_routes_it_on_resume() {
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
    let other = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_other")
        .replace("slot_work", "slot_other")
        .replace(
            "Return a short result",
            "archon-lab-work:400ms Return a short result",
        );
    let mission = format!(
        "{}\n[[formation]]{judge}\n[[formation]]{other}\n[[gate]]\nid = \"gate_human\"\ntitle = \"Human\"\nkinds = [\"human\"]\n",
        gated_mission("kinds = [\"formation\"]")
    ) + "\n[[connection]]\nid = \"edge_judge\"\nfrom = \"gate_review:judge\"\nto = \"fmn_judge:port_in\"\n[[connection]]\nid = \"edge_return\"\nfrom = \"fmn_judge:port_out\"\nto = \"gate_review:judge\"\n[[connection]]\nid = \"edge_other\"\nfrom = \"inp_hello:out\"\nto = \"fmn_other:port_in\"\n[[connection]]\nid = \"edge_human\"\nfrom = \"fmn_other:port_out\"\nto = \"gate_human:in\"\n[[connection]]\nid = \"edge_human_pass\"\nfrom = \"gate_human:pass\"\nto = \"end_done:in\"\n[[connection]]\nid = \"edge_human_fail\"\nfrom = \"gate_human:fail\"\nto = \"end_rejected:in\"\n";
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "no verdict".into())]),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let requested = loop {
        let p = c.run(&id).unwrap();
        if p.status == "blocked" {
            assert_eq!(p.block_code, "invalid_judge_result");
            break p
                .waiting_gates
                .iter()
                .find(|gate| gate.gate_id == "gate_human")
                .unwrap()
                .requested_seq;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    let view = c
        .record_verdict(
            &id,
            "gate_human",
            parvi_missions::HumanVerdict {
                requested_seq: requested,
                verdict: "pass".into(),
                response: "approved while blocked".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(view.status, "blocked");
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
            assert!(p.events.iter().any(|event| event.kind == "gate_verdict"
                && event.gate_id == "gate_human"
                && event.verdict == "pass"));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "no verdict".into())]),
            ..Default::default()
        })
        .unwrap();
    let requested = loop {
        let p = c.run(&id).unwrap();
        if p.status == "blocked" {
            break p
                .waiting_gates
                .iter()
                .find(|gate| gate.gate_id == "gate_human")
                .unwrap()
                .requested_seq;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
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
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("resume: {error}"),
        }
    }
    loop {
        let p = c.run(&id).unwrap();
        if p.status == "waiting_human" {
            assert!(!p.r#final);
            assert_eq!(p.waiting_gates[0].requested_seq, requested);
            break;
        }
        assert!(!p.r#final && p.status != "blocked" && Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    c.record_verdict(
        &id,
        "gate_human",
        parvi_missions::HumanVerdict {
            requested_seq: requested,
            verdict: "pass".into(),
            ..Default::default()
        },
    )
    .unwrap();
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final {
            assert_eq!(p.status, "failed");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn reopening_retries_an_open_attempt_on_its_original_inputs() {
    let root = Scratch::new();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "original".into())]),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !c.run(&id).unwrap().r#final {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(c);
    let path = root.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let events: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let start = events
        .iter()
        .position(|event| event["type"] == "node_started" && event["nodeId"] == "fmn_work")
        .unwrap();
    let kept = text.lines().take(start + 1).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(&path, kept).unwrap();
    let c = loop {
        match Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)) {
            Ok(c) => break c,
            Err(_) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" {
            assert_eq!(p.status, "succeeded");
            let retry = p
                .events
                .iter()
                .find(|event| {
                    event.kind == "node_started"
                        && event.node_id == "fmn_work"
                        && event.attempt == 2
                })
                .unwrap();
            assert_eq!(retry.input_refs.len(), 1);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn replay_blocks_instead_of_succeeding_when_an_output_payload_is_missing() {
    let root = Scratch::new();
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
    while !c.run(&id).unwrap().r#final {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(c);
    let path = root.0.join("runs").join(format!("{id}.ndjson"));
    let mut events: Vec<Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let output = events
        .iter()
        .position(|event| event["type"] == "node_output" && event["nodeId"] == "fmn_work")
        .unwrap();
    events.truncate(output + 1);
    events[output]["data"]["outputs"] = serde_json::json!({});
    std::fs::write(
        &path,
        events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let c = loop {
        match Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)) {
            Ok(c) => break c,
            Err(_) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" && p.block_code != "coordinator_interrupted" {
            assert_eq!(p.status, "succeeded");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    // Replay refused the payload-less output: the restart blocked the run
    // (and resumed it on its own) instead of succeeding from the replay.
    let events = c.run(&id).unwrap().events;
    let block = events.iter().position(|e| e.kind == "run_blocked").unwrap();
    assert!(events[block..].iter().any(|e| e.kind == "run_resumed"));
}

struct FailingLab;
impl parvi_missions::Executor for FailingLab {
    fn execute_formation(
        &self,
        request: parvi_missions::FormationExecution,
        events: parvi_missions::ExecutionSink,
    ) -> Result<parvi_missions::FormationResult, String> {
        let fail = request.formation.id == "fmn_work" && request.attempt == 1;
        let result = parvi_missions::Executor::execute_formation(&LabExecutor, request, events)?;
        if fail {
            Err("deliberate step error".into())
        } else {
            Ok(result)
        }
    }
}

#[test]
fn a_step_error_is_recorded_while_other_work_finishes_and_resume_retries_it() {
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let other = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_other")
        .replace("slot_work", "slot_other")
        .replace(
            "Return a short result",
            "archon-lab-work:400ms Return a short result",
        );
    let mission = solo.replace(
        "Return a short result",
        "archon-lab-work:50ms Return a short result",
    ) + &format!(
        "\n[[formation]]{other}\n[[connection]]\nid = \"edge_other\"\nfrom = \"inp_hello:out\"\nto = \"fmn_other:port_in\"\n[[connection]]\nid = \"edge_other_end\"\nfrom = \"fmn_other:port_out\"\nto = \"end_done:in\"\n"
    );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(FailingLab)).unwrap();
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
    let blocked = loop {
        let p = c.run(&id).unwrap();
        if p.status == "blocked" {
            break p;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(blocked.resume_allowed);
    let failure = blocked
        .events
        .iter()
        .find(|event| event.kind == "error" && event.node_id == "fmn_work")
        .unwrap()
        .seq;
    let other = blocked
        .events
        .iter()
        .find(|event| event.kind == "node_output" && event.node_id == "fmn_other")
        .unwrap()
        .seq;
    assert!(failure < other);
    loop {
        match c.resume(&id, Default::default()) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("resume: {error}"),
        }
    }
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final || p.status == "blocked" {
            assert_eq!(p.status, "succeeded");
            assert_eq!(
                p.events
                    .iter()
                    .filter(|event| event.kind == "node_started" && event.node_id == "fmn_other")
                    .count(),
                1
            );
            assert!(p.events.iter().any(|event| event.kind == "node_started"
                && event.node_id == "fmn_work"
                && event.attempt == 2));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn recovery_drains_started_work_without_retrying_a_recorded_error() {
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let other = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_other")
        .replace("slot_work", "slot_other")
        .replace(
            "Return a short result",
            "archon-lab-work:300ms Return a short result",
        );
    let mission = solo.replace(
        "Return a short result",
        "archon-lab-work:50ms Return a short result",
    ) + &format!(
        "\n[[formation]]{other}\n[[connection]]\nid = \"edge_other\"\nfrom = \"inp_hello:out\"\nto = \"fmn_other:port_in\"\n[[connection]]\nid = \"edge_other_end\"\nfrom = \"fmn_other:port_out\"\nto = \"end_done:in\"\n"
    );
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(FailingLab)).unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "hello".into())]),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while c.run(&id).unwrap().status != "blocked" {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(c);
    let path = root.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let events: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let failure = events
        .iter()
        .position(|e| e["type"] == "error" && e["nodeId"] == "fmn_work")
        .unwrap();
    assert!(
        events[..failure]
            .iter()
            .any(|e| e["type"] == "node_started" && e["nodeId"] == "fmn_other")
    );
    std::fs::write(
        &path,
        text.lines()
            .take(failure + 1)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let c = loop {
        match Coordinator::open(&root.0, RunSettings::default(), Box::new(LabExecutor)) {
            Ok(c) => break c,
            Err(_) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    loop {
        let p = c.run(&id).unwrap();
        if p.r#final {
            assert!(
                p.events
                    .iter()
                    .any(|e| e.kind == "node_output" && e.node_id == "fmn_other")
            );
            assert_eq!(
                p.events
                    .iter()
                    .filter(|e| e.kind == "node_started" && e.node_id == "fmn_work")
                    .count(),
                2,
                "status {} events {:?}",
                p.status,
                p.events
                    .iter()
                    .map(|e| (&e.kind, &e.node_id, e.attempt))
                    .collect::<Vec<_>>()
            );
            break;
        }
        assert!(!p.r#final && Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn human_response_is_delivered_with_its_original_input_like_archon() {
    let root = Scratch::new();
    let solo = include_str!("parity/runs/missions/solo-lab.mission.toml");
    let after = solo
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap()
        .replace("fmn_work", "fmn_after")
        .replace("slot_work", "slot_after");
    let mission = gated_mission("kinds = [\"human\"]")
        .replace("to = \"end_done:in\"", "to = \"fmn_after:port_in\"")
        + &format!(
            "\n[[formation]]{after}\n[[connection]]\nid = \"edge_after\"\nfrom = \"fmn_after:port_out\"\nto = \"end_done:in\"\n"
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
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    c.record_verdict(
        &id,
        "gate_review",
        parvi_missions::HumanVerdict {
            requested_seq: seq,
            verdict: "pass".into(),
            response: "yes".into(),
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
    let expected: Vec<Value> = include_str!("parity/runs/archon-ledgers/human-response.ndjson")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(normalized_ledger(&root, &id), expected);
    run_support::assert_payload_parity(
        &root,
        &id,
        include_str!("parity/runs/archon-ledgers/human-response.payload.ndjson"),
    );
}
