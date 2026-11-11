use parvi_node_kit::{Built, Listing, Setup};
use parvi_testkit::{node::Node, sessions};
use serde_json::json;

fn lab(setup: &Setup) -> anyhow::Result<Built> {
    parvi_missions::build_with_executor(setup, Some(Box::new(parvi_mission_engine::LabExecutor)))
}
const LAB: Listing = Listing {
    build: lab,
    ..parvi_missions::LISTING
};
const EXPECTED: &[&str] = &[
    "mission_list",
    "mission_inspect",
    "mission_validate",
    "mission_new",
    "mission_edit",
    "mission_notes",
    "mission_note",
    "layout_show",
    "layout_edit",
    "mission_run",
    "formation_run",
    "run_list",
    "run_status",
    "run_wait",
    "run_seats",
    "run_logs",
    "run_abort",
    "run_resume",
    "run_land",
    "gate_approve",
    "gate_reject",
    "gate_pick",
];

#[tokio::test(flavor = "multi_thread")]
async fn command_list_and_coded_failures() {
    assert_eq!(parvi_missions::LISTING.commands, EXPECTED);
    let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, LAB], "")
        .await
        .unwrap();
    for &command in EXPECTED {
        let args = match command {
            "mission_list" => json!({"unexpected":true}),
            "run_list" => json!({"mission":9}),
            _ => json!({}),
        };
        let error = node.call(command, args).await.unwrap_err();
        assert_eq!(error.code, "bad_request", "{command}: {error}");
    }
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn author_and_run_with_tools() {
    let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, LAB], "")
        .await
        .unwrap();
    assert!(node.call("mission_list", json!({})).await.unwrap()["missions"].is_array());
    node.call("mission_new", json!({"slug":"demo","title":"Demo"}))
        .await
        .unwrap();
    assert_eq!(
        node.call("mission_inspect", json!({"mission":"demo"}))
            .await
            .unwrap()["mission"]["title"],
        "Demo"
    );
    assert!(
        node.call("mission_validate", json!({"mission":"demo"}))
            .await
            .unwrap()["errors"]
            .is_array()
    );
    let edited = node
        .call(
            "mission_edit",
            json!({"mission":"demo","change":{"createInputCard":{"title":"Start","goal":"Work"}}}),
        )
        .await
        .unwrap();
    let id = edited["inputCard"]["id"].as_str().unwrap();
    assert!(!id.is_empty());
    let note = node
        .call("mission_note", json!({"mission":"demo","text":"intent"}))
        .await
        .unwrap();
    assert_eq!(note["notes"]["mission"][0]["text"], "intent");
    assert_eq!(
        node.call("mission_notes", json!({"mission":"demo"}))
            .await
            .unwrap()["notes"]["mission"][0]["text"],
        "intent"
    );
    assert!(
        node.call("layout_show", json!({"mission":"demo"}))
            .await
            .unwrap()["layout"]["nodes"]
            .is_array()
    );
    assert_eq!(
        node.call(
            "layout_edit",
            json!({"mission":"demo","change":{"nodes":[{"id":id,"x":10,"y":20}]}})
        )
        .await
        .unwrap()["layout"]["nodes"][0]["x"],
        10
    );
    let folder = node.dir().join("missions/missions");
    std::fs::write(
        folder.join("hello.mission.toml"),
        include_str!("fixtures/solo-lab.mission.toml"),
    )
    .unwrap();
    std::fs::write(
        folder.join("admission-findings.mission.toml"),
        include_str!("fixtures/admission-findings.mission.toml"),
    )
    .unwrap();
    let rejected = node
        .call("mission_run", json!({"mission":"admission-findings"}))
        .await
        .unwrap_err();
    assert_eq!(rejected.code, "run_admission_failed");
    assert!(
        rejected.message.contains("missing_input")
            && rejected.message.contains("formation_without_slots")
    );
    let started = node
        .call(
            "mission_run",
            json!({"mission":"hello","inputs":{"brief":"hi"}}),
        )
        .await
        .unwrap();
    let run = started["runId"].as_str().unwrap();
    assert!(node.call("run_list", json!({})).await.unwrap()["runs"].is_array());
    assert_eq!(
        node.call("run_wait", json!({"run":run,"seconds":5}))
            .await
            .unwrap()["final"],
        true
    );
    assert_eq!(
        node.call("run_status", json!({"run":run})).await.unwrap()["runId"],
        run
    );
    assert!(node.call("run_logs", json!({"run":run})).await.unwrap()["events"].is_array());
    assert!(node.call("run_seats", json!({"run":run})).await.unwrap()["seats"].is_array());
    // A durable, resumable run: copy the engine's actual snapshots from the
    // completed run, then record a shutdown block for a fresh run id.
    let runs = node.dir().join("missions/runs");
    let original = parvi_mission_engine::Ledger::new(runs.join(format!("{run}.ndjson")), run);
    let mut first = original.read().unwrap().remove(0);
    first.run_id.clear();
    let resumed = "run_resume_test";
    for suffix in ["snapshot.toml", "bindings.toml"] {
        std::fs::copy(
            runs.join(format!("{run}.{suffix}")),
            runs.join(format!("{resumed}.{suffix}")),
        )
        .unwrap();
    }
    let ledger = parvi_mission_engine::Ledger::new(runs.join(format!("{resumed}.ndjson")), resumed);
    ledger.create(first).unwrap();
    let blocked = parvi_mission_engine::RunEvent {
        kind: "run_blocked".into(),
        timestamp: "2026-01-01T00:00:00Z".into(),
        data: json!({"code":"coordinator_shutdown","resumeAllowed":true})
            .as_object()
            .unwrap()
            .clone(),
        ..Default::default()
    };
    ledger.append(blocked).unwrap();
    let response = node
        .call("run_resume", json!({"run":resumed}))
        .await
        .unwrap();
    assert_eq!(response["run"]["runId"], resumed);
    assert!(
        ledger
            .read()
            .unwrap()
            .iter()
            .any(|event| event.kind == "run_resumed")
    );
    let formation = node
        .call(
            "formation_run",
            json!({"mission":"hello","formation":"fmn_work","inputs":{"brief":"hi"}}),
        )
        .await
        .unwrap();
    assert!(formation["runId"].is_string());
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn gate_and_abort_commands_act_on_pending_runs() {
    use parvi_mission_engine::{Ledger, RunEvent};
    let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, LAB], "")
        .await
        .unwrap();
    let event = |kind: &str| RunEvent {
        kind: kind.into(),
        actor: "agent:test".into(),
        timestamp: "2026-01-01T00:00:00Z".into(),
        ..Default::default()
    };
    for (run, command, pick) in [
        ("run_approve", "gate_approve", false),
        ("run_reject", "gate_reject", false),
        ("run_pick", "gate_pick", true),
    ] {
        let ledger = Ledger::new(
            node.dir()
                .join("missions/runs")
                .join(format!("{run}.ndjson")),
            run,
        );
        ledger.create(event("run_started")).unwrap();
        let mut evaluating = event("gate_evaluating");
        evaluating.gate_id = "review".into();
        ledger.append(evaluating).unwrap();
        let mut request = event("human_input_requested");
        request.gate_id = "review".into();
        if pick {
            request.data = json!({"pick":{"candidates":[1,2]}})
                .as_object()
                .unwrap()
                .clone();
        }
        ledger.append(request).unwrap();
        let args = if pick {
            json!({"run":run,"gate":"review","requested_seq":3,"attempt":1})
        } else {
            json!({"run":run,"gate":"review","requested_seq":3})
        };
        assert_eq!(node.call(command, args).await.unwrap()["runId"], run);
    }
    let ledger = Ledger::new(
        node.dir().join("missions/runs/run_abort.ndjson"),
        "run_abort",
    );
    ledger.create(event("run_started")).unwrap();
    assert_eq!(
        node.call("run_abort", json!({"run":"run_abort"}))
            .await
            .unwrap()["run"]["status"],
        "canceled"
    );
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repository_run_can_be_landed() {
    let root = parvi_testkit::fresh_dir("mission-checkout");
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "test@example.invalid"]);
    std::fs::write(root.join("readme"), "test\n").unwrap();
    git(&["add", "readme"]);
    git(&["commit", "-m", "initial"]);
    let config = format!("[missions]\nrepositories = {{ \"example/project\" = {root:?} }}\n");
    let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, LAB], &config)
        .await
        .unwrap();
    let folder = node.dir().join("missions/missions");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("hello.mission.toml"),
        include_str!("fixtures/solo-lab.mission.toml"),
    )
    .unwrap();
    let start = node
        .call(
            "mission_run",
            json!({"mission":"hello","repository":"example/project","inputs":{"brief":"hi"}}),
        )
        .await
        .unwrap();
    let run = start["runId"].as_str().unwrap();
    assert_eq!(
        node.call("run_wait", json!({"run":run,"seconds":5}))
            .await
            .unwrap()["final"],
        true
    );
    assert_eq!(
        node.call("run_land", json!({"run":run,"force":true}))
            .await
            .unwrap()["landed"],
        true
    );
    node.stop().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_uses_the_same_mission_commands() {
    let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, LAB], "")
        .await
        .unwrap();
    let mut output = Vec::new();
    let args = vec![
        "new".to_string(),
        "demo".to_string(),
        "--node".to_string(),
        format!("{}/", node.url()),
        "--json".to_string(),
    ];
    assert_eq!(
        parvi_node_kit::cli::run("mission", &args, &mut output)
            .await
            .unwrap(),
        0
    );
    let result: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(result["mission"]["slug"], "demo");
    node.stop().await;
}
