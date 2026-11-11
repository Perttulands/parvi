//! The area exposes the old Work commands and only pushes while watched.
use futures_util::{SinkExt, StreamExt};
use parvi_testkit::node::Node;
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[test]
fn commands_match_the_old_cli_and_mcp_tools() {
    assert_eq!(
        parvi_work::LISTING.commands,
        &["work_list", "work_show", "work_create"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn work_is_pushed_only_while_watched() {
    let node = Node::start(&[parvi_work::LISTING], "").await.unwrap();
    let answer = node.call("work_list", json!({})).await.unwrap();
    assert_eq!(answer["projects"], json!([]));
    assert!(node.call("work_show", json!({"ref":"bad"})).await.is_err());
    let (mut ws, _) = tokio_tungstenite::connect_async(node.ws()).await.unwrap();
    ws.next().await.unwrap().unwrap(); // hello
    assert!(
        tokio::time::timeout(Duration::from_millis(50), ws.next())
            .await
            .is_err()
    );
    ws.send(Message::Binary(vec![0x98, 1].into()))
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_data();
    assert_eq!(&frame[..], b"\x90{\"projects\":[]}");
    ws.send(Message::Binary(vec![0x98, 0].into()))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), ws.next())
            .await
            .is_err()
    );
    drop(ws);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_from_cli_files_a_bead_in_the_configured_store() {
    let dir = parvi_testkit::fresh_dir("parvi-work-create");
    let bd = |args: &[&str]| {
        std::process::Command::new("bd")
            .args(args)
            .current_dir(&dir)
            .env_remove("BEADS_DIR")
            .env_remove("BEADS_DB")
            .env_remove("BD_DB")
            .output()
            .unwrap()
    };
    let init = bd(&[
        "init",
        "--prefix",
        "pwc",
        "--skip-agents",
        "--skip-hooks",
        "--non-interactive",
    ]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let config = format!("[work]\nprojects = [\"beads {}\"]\n", dir.display());
    let node = Node::start(&[parvi_work::LISTING], &config).await.unwrap();
    let missing: Vec<String> = [
        "create",
        "--project",
        "missing",
        "--title",
        "T",
        "--description",
        "D",
        "--node",
        &node.url(),
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        parvi_node_kit::cli::run("work", &missing, &mut Vec::new())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        node.call(
            "work_create",
            json!({"project":"missing", "title":"T", "description":"D"})
        )
        .await
        .unwrap_err()
        .code,
        "unknown_project"
    );
    let client = parvi_node_kit::NodeClient::new(Some(&node.url())).unwrap();
    let (created, _) = client.request(axum::http::Method::POST, "/api/work/items", Some(json!({"project": dir.display().to_string(), "title":"Via HTTP", "description":"Body", "type":"feature"})), None).await.unwrap();
    let http_id = created["reference"]
        .as_str()
        .unwrap()
        .rsplit('#')
        .next()
        .unwrap();
    let shown = bd(&["--json", "show", http_id]);
    let item: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(item[0]["title"], "Via HTTP");
    assert_eq!(item[0]["issue_type"], "feature");
    let args: Vec<String> = vec![
        "create".into(),
        "--node".into(),
        node.url(),
        "--project".into(),
        dir.display().to_string(),
        "--title".into(),
        "T".into(),
        "--description".into(),
        "D".into(),
    ];
    let mut out = Vec::new();
    assert_eq!(
        parvi_node_kit::cli::run("work", &args, &mut out)
            .await
            .unwrap(),
        0
    );
    let printed = String::from_utf8(out).unwrap();
    let id = printed.trim().rsplit('#').next().unwrap();
    assert!(id.starts_with("pwc-"), "{printed}");
    let shown = bd(&["--json", "show", id]);
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(value[0]["title"], "T");
    assert_eq!(value[0]["description"], "D");
    assert_eq!(value[0]["issue_type"], "bug");
    node.stop().await;
    std::fs::remove_dir_all(dir).unwrap();
}
