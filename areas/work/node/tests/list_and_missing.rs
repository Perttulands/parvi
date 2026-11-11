//! Work commands filter their own response without changing the page's board.
#![cfg(unix)]
use futures_util::{SinkExt, StreamExt};
use parvi_node_kit::{Built, Listing, Setup};
use parvi_testkit::node::Node;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::sync::OnceLock;
use tokio_tungstenite::tungstenite::Message;

static BD: OnceLock<String> = OnceLock::new();
fn build(setup: &Setup) -> anyhow::Result<Built> {
    parvi_work::build_with(
        setup,
        parvi_work::Bd::new(BD.get().unwrap()),
        parvi_work::Gh::default(),
    )
}
const WORK: Listing = Listing {
    build,
    ..parvi_work::LISTING
};

#[tokio::test(flavor = "multi_thread")]
async fn list_filters_closed_without_changing_the_page_and_missing_is_not_found() {
    let dir = parvi_testkit::fresh_dir("parvi-work-filter");
    std::fs::create_dir(dir.join(".beads")).unwrap();
    std::fs::write(
        dir.join(".beads/list.json"),
        include_str!("fixtures/work/beads.json"),
    )
    .unwrap();
    let program = dir.join("fake-bd");
    std::fs::write(&program, "#!/bin/sh\ncase \"$2\" in config) printf '{\"value\":\"tst\"}';; list) cat .beads/list.json;; *) exit 1;; esac\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    BD.set(program.display().to_string()).unwrap();
    let node = Node::start(
        &[WORK],
        &format!("[work]\nprojects = [\"beads {}\"]\n", dir.display()),
    )
    .await
    .unwrap();

    let list = node.call("work_list", json!({})).await.unwrap();
    let ids = |v: &Value| {
        v.as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&list["items"]).len(), 3);
    assert!(!ids(&list["items"]).contains(&"tst-1mt".to_owned()));
    assert!(!ids(&list["projects"][0]["items"]).contains(&"tst-1mt".to_owned()));
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["status"] == "in_progress")
    );
    let mut out = Vec::new();
    let args = ["list", "--all", "--json", "--node", &node.url()].map(str::to_owned);
    assert_eq!(
        parvi_node_kit::cli::run("work", &args, &mut out)
            .await
            .unwrap(),
        0
    );
    let all: Value = serde_json::from_slice(&out).unwrap();
    assert!(ids(&all["items"]).contains(&"tst-1mt".to_owned()));
    assert!(ids(&all["projects"][0]["items"]).contains(&"tst-1mt".to_owned()));

    let (mut ws, _) = tokio_tungstenite::connect_async(node.ws()).await.unwrap();
    ws.next().await.unwrap().unwrap();
    ws.send(Message::Binary(vec![0x98, 1].into()))
        .await
        .unwrap();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let frame = ws.next().await.unwrap().unwrap().into_data();
            if frame.first() == Some(&0x90) {
                let board: Value = serde_json::from_slice(&frame[1..]).unwrap();
                if board["projects"][0]["items"].is_array() {
                    break board;
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(ids(&frame["projects"][0]["items"]).contains(&"tst-1mt".to_owned()));
    let error = node
        .call("work_show", json!({"ref":"tst-missing"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "not_found");
    assert!(error.message.contains("tst-missing"));
    let client = parvi_node_kit::NodeClient::new(Some(&node.url())).unwrap();
    let error = client
        .request(
            axum::http::Method::GET,
            "/api/work/item?ref=tst-missing",
            None,
            None,
        )
        .await
        .unwrap_err();
    let error = error.downcast_ref::<parvi_node_kit::NodeError>().unwrap();
    assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(error.code, "not_found");
    drop(ws);
    node.stop().await;
    std::fs::remove_dir_all(dir).unwrap();
}
