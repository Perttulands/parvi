//! A two-step mission authored and driven through the same CLI as an operator.
use parvi_mission_engine::{
    ExecutionSink, Executor, FormationExecution, FormationResult, LabExecutor,
};
use parvi_node_kit::{Built, Listing, Setup};
use parvi_term::Sessions;
use parvi_testkit::{
    node::Node,
    sessions::{self, FakeSessions},
};
use serde_json::Value;
use std::sync::{Arc, Mutex};

// The executor delivers each step to the testkit's fake Sessions, then uses
// the deterministic lab answer. It runs no real terminal or model.
static FAKE: Mutex<Option<Arc<FakeSessions>>> = Mutex::new(None);
struct FakeSeatLab {
    runtime: tokio::runtime::Handle,
}
impl Executor for FakeSeatLab {
    fn execute_formation(
        &self,
        execution: FormationExecution,
        sink: ExecutionSink,
    ) -> Result<FormationResult, parvi_core::Error> {
        let sessions = FAKE
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| parvi_core::Error::new("test", "fake Sessions unavailable"))?;
        let id = sessions.add(&format!("seat-{}", execution.formation.id));
        self.runtime
            .block_on(async {
                let pane = sessions.active_pane(&id).await?;
                sessions
                    .deliver(pane, execution.input_text.clone(), true)
                    .await?;
                Ok::<_, anyhow::Error>(())
            })
            .map_err(|e| parvi_core::Error::new("test", e.to_string()))?;
        LabExecutor.execute_formation(execution, sink)
    }
}
fn build(setup: &Setup) -> anyhow::Result<Built> {
    parvi_missions::build_with_executor(
        setup,
        Some(Box::new(FakeSeatLab {
            runtime: tokio::runtime::Handle::current(),
        })),
    )
}
const MISSIONS: Listing = Listing {
    build,
    ..parvi_missions::LISTING
};
async fn cli(node: &Node, noun: &str, args: &[&str]) -> Value {
    let mut args: Vec<String> = args.iter().map(|s| (*s).into()).collect();
    args.extend(["--node".into(), format!("{}/", node.url()), "--json".into()]);
    let mut output = Vec::new();
    let exit = parvi_node_kit::cli::run(noun, &args, &mut output)
        .await
        .unwrap();
    assert_eq!(exit, 0, "{noun} {args:?}");
    serde_json::from_slice(&output).unwrap()
}
#[tokio::test(flavor = "multi_thread")]
async fn cli_authors_two_steps_answers_gate_and_finishes_with_fake_sessions() {
    let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, MISSIONS], "")
        .await
        .unwrap();
    let fake = node.fake_sessions();
    *FAKE.lock().unwrap() = Some(fake.clone());
    cli(&node, "mission", &["new", "relay", "--title", "Relay"]).await;
    let card = cli(
        &node,
        "mission",
        &[
            "create",
            "relay",
            "--title",
            "Start",
            "--goal",
            "Relay the brief",
        ],
    )
    .await["inputCard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut steps = Vec::new();
    for title in ["Draft", "Polish"] {
        let result = cli(
            &node,
            "formation",
            &["create", "relay", "--type", "solo", "--title", title],
        )
        .await;
        let step = &result["formation"];
        let id = step["id"].as_str().unwrap().to_owned();
        let slot = step["slots"][0]["id"].as_str().unwrap();
        cli(
            &node,
            "formation",
            &[
                "assign",
                "relay",
                &id,
                "--slot",
                slot,
                "--harness",
                "openai-codex",
                "--effort",
                "medium",
            ],
        )
        .await;
        cli(
            &node,
            "formation",
            &["set-brief", "relay", &id, "--goal", title],
        )
        .await;
        steps.push((
            format!("{id}:{}", step["inputs"][0]["id"].as_str().unwrap()),
            format!("{id}:{}", step["outputs"][0]["id"].as_str().unwrap()),
            id,
        ));
    }
    let gate = cli(
        &node,
        "gate",
        &[
            "create",
            "relay",
            "--title",
            "Review",
            "--criterion",
            "Is this ready?",
        ],
    )
    .await["gate"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let done = cli(&node, "end", &["create", "relay", "--outcome", "done"]).await["end"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let rejected =
        cli(&node, "end", &["create", "relay", "--outcome", "rejected"]).await["end"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
    cli(
        &node,
        "mission",
        &["wire", "relay", &steps[0].0, "--input", &card],
    )
    .await;
    for (from, to) in [
        (&steps[0].1, format!("{gate}:in")),
        (&format!("{gate}:pass"), steps[1].0.clone()),
        (&format!("{gate}:fail"), format!("{rejected}:in")),
        (&steps[1].1, format!("{done}:in")),
    ] {
        cli(&node, "formation", &["wire", "relay", from, &to]).await;
    }
    let started = cli(
        &node,
        "mission",
        &["run", "relay", "--inputs", r#"{"brief":"hello"}"#],
    )
    .await;
    let run = started["runId"].as_str().unwrap();
    let waiting = cli(&node, "run", &["wait", run, "--timeout", "10"]).await;
    assert_eq!(waiting["outcome"], "needs-you", "{waiting}");
    assert_eq!(waiting["asks"][0]["gate"], gate);
    let gates = cli(&node, "run", &["gates", run]).await;
    assert_eq!(gates["gates"][0]["state"], "waiting");
    let request = cli(&node, "gate", &["request", run, &gate]).await;
    assert_eq!(request["criterion"], "Is this ready?");
    assert_eq!(request["inputs"][0]["fromNodeId"], steps[0].2);
    assert_eq!(request["routes"][0]["targets"][0]["nodeId"], steps[1].2);
    assert_eq!(request["routes"][0]["endsRun"], false);
    assert_eq!(request["routes"][1]["targets"][0]["nodeId"], rejected);
    assert_eq!(request["routes"][1]["endsRun"], true);
    assert_eq!(request["endsRun"], true);
    assert_eq!(request["picksBest"], false);
    let cursor = waiting["cursor"].as_u64().unwrap();
    let status = cli(&node, "run", &["status", run]).await;
    let seq = status["waitingGates"][0]["requestedSeq"].as_u64().unwrap();
    let expected = format!("parvi gate approve {run} {gate} --requested-seq {seq}");
    assert!(
        waiting["next"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == &expected)
    );
    let mut output = Vec::new();
    let args = vec![
        "wait".into(),
        run.into(),
        "--until".into(),
        "needs-you".into(),
        "--node".into(),
        format!("{}/", node.url()),
    ];
    assert_eq!(
        parvi_node_kit::cli::run("run", &args, &mut output)
            .await
            .unwrap(),
        0
    );
    assert!(String::from_utf8(output).unwrap().contains(&expected));
    let timed = cli(
        &node,
        "run",
        &[
            "wait",
            run,
            "--since",
            &cursor.to_string(),
            "--timeout",
            "0",
        ],
    )
    .await;
    assert_eq!(timed["outcome"], "timeout");
    assert_eq!(timed["status"], "waiting_human");
    cli(
        &node,
        "gate",
        &[
            "approve",
            run,
            &gate,
            &seq.to_string(),
            "--response",
            "approved",
        ],
    )
    .await;
    let gates = cli(&node, "run", &["gates", run]).await;
    assert_eq!(gates["gates"][0]["state"], "answered");
    assert_eq!(gates["gates"][0]["verdict"], "pass");
    assert_eq!(gates["gates"][0]["by"], "human:operator");
    let error = node
        .call("gate_request", serde_json::json!({"run":run,"gate":gate}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "gate_not_waiting");
    let mut output = Vec::new();
    let args = vec![
        "request".into(),
        run.into(),
        gate.clone(),
        "--node".into(),
        format!("{}/", node.url()),
        "--json".into(),
    ];
    assert_eq!(
        parvi_node_kit::cli::run("gate", &args, &mut output)
            .await
            .unwrap(),
        1
    );
    let finished = cli(
        &node,
        "run",
        &["wait", run, "--until", "final", "--timeout", "10"],
    )
    .await;
    assert_eq!(finished["outcome"], "final");
    assert_eq!(finished["status"], "succeeded", "{finished}");
    let again = cli(
        &node,
        "run",
        &["wait", run, "--until", "final", "--timeout", "0"],
    )
    .await;
    assert_eq!(again["outcome"], "final");
    let status = cli(&node, "run", &["status", run]).await;
    for (_, _, id) in &steps {
        assert!(
            status["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["type"] == "node_output" && e["nodeId"] == *id),
            "{status}"
        );
    }
    let delivered = fake.delivered();
    assert_eq!(delivered.len(), 2);
    assert!(delivered.iter().all(|d| d.enter));
    *FAKE.lock().unwrap() = None;
    node.stop().await;
}
