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
use std::time::Duration;

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
    let gate = cli(&node, "gate", &["create", "relay", "--title", "Review"]).await["gate"]["id"]
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
    let seq = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = cli(&node, "run", &["status", run]).await;
            if let Some(seq) = status["waitingGates"][0]["requestedSeq"].as_u64() {
                break seq;
            }
            assert_ne!(status["final"], true, "{status}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("run reaches human gate");
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
    let finished = cli(&node, "run", &["wait", run, "--seconds", "10"]).await;
    assert_eq!(finished["status"], "succeeded", "{finished}");
    for (_, _, id) in &steps {
        assert!(
            finished["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["type"] == "node_output" && e["nodeId"] == *id),
            "{finished}"
        );
    }
    let delivered = fake.delivered();
    assert_eq!(delivered.len(), 2);
    assert!(delivered.iter().all(|d| d.enter));
    *FAKE.lock().unwrap() = None;
    node.stop().await;
}
