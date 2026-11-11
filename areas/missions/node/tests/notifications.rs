#![cfg(unix)]
use parvi_node_kit::{Built, Config, Listing, Options, Running, Setup};
use parvi_testkit::sessions;
use serde_json::Value;
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

static LOG: Mutex<Option<Arc<parvi_node_kit::events::Log>>> = Mutex::new(None);
fn lab(setup: &Setup) -> anyhow::Result<Built> {
    *LOG.lock().unwrap() = Some(setup.events.clone());
    parvi_missions::build_with_executor(setup, Some(Box::new(parvi_mission_engine::LabExecutor)))
}
const LAB: Listing = Listing {
    build: lab,
    ..parvi_missions::LISTING
};
async fn start(root: &Path, config: &str) -> Running {
    parvi_node_kit::start(
        Options {
            dir: root.into(),
            config: Config::parse(config, root.into()).unwrap(),
            listen: Some("127.0.0.1:0".into()),
        },
        &[sessions::LISTING, parvi_work::LISTING, LAB],
    )
    .await
    .unwrap()
}
async fn cli(node: &Running, noun: &str, args: &[&str]) -> Value {
    let mut args: Vec<String> = args.iter().map(|arg| (*arg).into()).collect();
    args.extend([
        "--node".into(),
        format!("http://{}/", node.address),
        "--json".into(),
    ]);
    let mut out = Vec::new();
    assert_eq!(
        parvi_node_kit::cli::run(noun, &args, &mut out)
            .await
            .unwrap(),
        0
    );
    serde_json::from_slice(&out).unwrap()
}
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test(flavor = "multi_thread")]
async fn node_restart_does_not_resend_and_old_final_runs_are_ignored() {
    let root = parvi_testkit::fresh_dir("notify-restart-node");
    let folder = root.join("missions/missions");
    std::fs::create_dir_all(&folder).unwrap();
    let base = include_str!(
        "../../../../engine/missions/tests/parity/runs/missions/solo-lab.mission.toml"
    );
    std::fs::write(folder.join("hello.mission.toml"), base).unwrap();
    let human = base.replace("msn_hello", "msn_review").replace("slug = \"hello\"", "slug = \"review\"")
        .replace("[[end]]", "[[gate]]\nid = \"gate_review\"\ntitle = \"Review\"\nkinds = [\"human\"]\ncriterion = \"Ready?\"\n\n[[end]]")
        .replace("to = \"end_done:in\"", "to = \"gate_review:in\"\n\n[[connection]]\nid = \"edge_pass\"\nfrom = \"gate_review:pass\"\nto = \"end_done:in\"\n\n[[end]]\nid = \"end_rejected\"\ntitle = \"Rejected\"\noutcome = \"rejected\"\n\n[[connection]]\nid = \"edge_fail\"\nfrom = \"gate_review:fail\"\nto = \"end_rejected:in\"");
    std::fs::write(folder.join("review.mission.toml"), human).unwrap();
    let first = start(&root, "").await;
    let old = cli(
        &first,
        "mission",
        &["run", "hello", "--inputs", r#"{"brief":"hi"}"#],
    )
    .await;
    let old_id = old["runId"].as_str().unwrap();
    assert_eq!(
        cli(
            &first,
            "run",
            &["wait", old_id, "--until", "final", "--timeout", "10"]
        )
        .await["outcome"],
        "final"
    );
    first.stop().await.unwrap();
    let command = root.join("notify");
    let output = root.join("sent.jsonl");
    std::fs::write(
        &command,
        format!(
            "#!/bin/sh\ncat >> '{}'\nprintf '\\n' >> '{}'\n",
            output.display(),
            output.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = format!("[missions]\nnotify_command = '{}'\n", command.display());
    let second = start(&root, &config).await;
    let started = cli(
        &second,
        "mission",
        &["run", "review", "--inputs", r#"{"brief":"hi"}"#],
    )
    .await;
    let id = started["runId"].as_str().unwrap();
    assert_eq!(
        cli(&second, "run", &["wait", id, "--timeout", "10"]).await["outcome"],
        "needs-you"
    );
    until(|| std::fs::read_to_string(&output).is_ok_and(|text| text.ends_with('\n'))).await;
    let before = std::fs::read_to_string(&output).unwrap();
    let messages: Vec<Value> = before
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["runId"], id);
    assert_eq!(messages[0]["missionSlug"], "review");
    assert_eq!(messages[0]["gateId"], "gate_review");
    assert_eq!(messages[0]["ask"], "Ready?");
    // Wait for the durable receipt, not merely the child writing its output.
    until(|| {
        root.join("missions/runs")
            .join(format!("{id}.needs-you.json"))
            .exists()
    })
    .await;
    second.stop().await.unwrap();
    let third = start(&root, &config).await;
    until(|| {
        LOG.lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .listen()
            .0
            .iter()
            .any(|event| event.text.starts_with("run needs you:"))
    })
    .await;
    third.stop().await.unwrap();
    assert_eq!(std::fs::read_to_string(&output).unwrap(), before);
    *LOG.lock().unwrap() = None;
    std::fs::remove_dir_all(root).unwrap();
}
