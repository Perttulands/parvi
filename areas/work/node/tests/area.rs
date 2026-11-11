//! The area exposes the old Work commands and only pushes while watched.
use futures_util::{SinkExt, StreamExt};
use parvi_node_kit::{Built, Listing, Setup};
use parvi_testkit::node::Node;
use serde_json::json;
use std::sync::OnceLock;
use std::time::Duration;
static BD: OnceLock<String> = OnceLock::new();
fn build_work(setup: &Setup) -> anyhow::Result<Built> {
    parvi_work::build_with(
        setup,
        parvi_work::Bd::new(BD.get().unwrap()),
        parvi_work::Gh::default(),
    )
}
const WORK: Listing = Listing {
    build: build_work,
    ..parvi_work::LISTING
};
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

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn cli_lists_shows_and_creates_a_work_item() {
    let dir = parvi_testkit::fresh_dir("parvi-work-create");
    std::fs::create_dir(dir.join(".beads")).unwrap();
    std::fs::write(dir.join(".beads/prefix"), "pwc").unwrap();
    let program = dir.join("fake-bd");
    std::fs::write(&program, include_str!("fixtures/work/fake-bd.sh")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    BD.set(program.display().to_string()).unwrap();
    let config = format!("[work]\nprojects = [\"beads {}\"]\n", dir.display());
    let node = Node::start(&[WORK], &config).await.unwrap();
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
    let http = dir.join(".beads").join(http_id);
    assert_eq!(
        std::fs::read_to_string(http.join("title")).unwrap(),
        "Via HTTP"
    );
    assert_eq!(
        std::fs::read_to_string(http.join("description")).unwrap(),
        "Body"
    );
    assert_eq!(
        std::fs::read_to_string(http.join("type")).unwrap(),
        "feature"
    );
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
    let filed = dir.join(".beads").join(id);
    assert_eq!(std::fs::read_to_string(filed.join("title")).unwrap(), "T");
    assert_eq!(
        std::fs::read_to_string(filed.join("description")).unwrap(),
        "D"
    );
    assert_eq!(std::fs::read_to_string(filed.join("type")).unwrap(), "bug");
    let list = vec!["list".into(), "--node".into(), node.url()];
    let mut out = Vec::new();
    assert_eq!(
        parvi_node_kit::cli::run("work", &list, &mut out)
            .await
            .unwrap(),
        0
    );
    assert!(String::from_utf8(out).unwrap().contains(id));
    let show = vec![
        "show".into(),
        "--node".into(),
        node.url(),
        "--ref".into(),
        id.into(),
    ];
    let mut out = Vec::new();
    assert_eq!(
        parvi_node_kit::cli::run("work", &show, &mut out)
            .await
            .unwrap(),
        0
    );
    let shown = String::from_utf8(out).unwrap();
    assert!(shown.contains("T") && shown.contains(id));

    node.stop().await;
    std::fs::remove_dir_all(dir).unwrap();
}
