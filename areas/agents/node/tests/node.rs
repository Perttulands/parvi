//! The Agents commands and channel on an isolated node and scratch home.
use std::time::Duration;

use axum::http::Method;
use futures_util::{SinkExt, StreamExt};
use parvi_agents_contract::{FAILED, REPORT, SYNC, SYNCED, WATCH};
use parvi_core::Error;
use parvi_node_kit::{NodeClient, cli};
use parvi_testkit::{fresh_dir, node::Node, sessions};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test(flavor = "multi_thread")]
async fn commands_and_channel_work_over_the_node() {
    let home = fresh_dir("parvi-agents-node-home");
    let skill = home.join(".agents/skills/example/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(
        &skill,
        "---\nname: example\ndescription: A fixture skill.\n---\n",
    )
    .unwrap();
    let config = format!("[agents]\nhome = {:?}\npath = []\n", home);
    let node = Node::start(&[sessions::LISTING, parvi_agents::LISTING], &config)
        .await
        .unwrap();
    let client = NodeClient::new(Some(&node.url())).unwrap();

    let (health, _) = client
        .request(Method::GET, "/api/health", None, None)
        .await
        .unwrap();
    assert_eq!(health["status"], "ok");
    assert!(
        !health["off"]
            .as_array()
            .unwrap()
            .iter()
            .any(|area| area == "agents")
    );
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"tools/list", "params":{}});
    let (list, _) = client
        .request(Method::POST, "/mcp", Some(request), None)
        .await
        .unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .filter(|name| name.starts_with("agents_"))
        .collect();
    assert_eq!(names, ["agents_show", "agents_sync"]);
    assert_eq!(parvi_agents::LISTING.commands, names);

    let args = vec!["--node".into(), node.url()];
    let mut output = Vec::new();
    assert_eq!(cli::run("agents", &args, &mut output).await.unwrap(), 0);
    let output = String::from_utf8(output).unwrap();
    assert!(
        output.contains("show") && output.contains("sync"),
        "{output}"
    );
    let report = node.call("agents_show", json!({})).await.unwrap();
    assert_eq!(report["node"], node.id());
    assert_eq!(report["skills"][0]["name"], "example");
    let error = node
        .call("agents_sync", json!({"checkout":"~/not-a-checkout"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, FAILED);
    assert!(error.message.contains("not a checkout"));
    let remote = node
        .call("agents_show", json!({"node":"another-node"}))
        .await
        .unwrap_err();
    assert_eq!(remote.code, "unknown_node");

    let (mut ws, _) = tokio_tungstenite::connect_async(node.ws()).await.unwrap();
    ws.send(Message::Binary(vec![WATCH, 1].into()))
        .await
        .unwrap();
    let frame = next_kind(&mut ws, REPORT).await;
    let report: Value = serde_json::from_slice(&frame[1..]).unwrap();
    assert_eq!(report["skills"][0]["name"], "example");
    ws.send(Message::Binary(
        [&[SYNC][..], &42_u32.to_le_bytes(), b"~/not-a-checkout"]
            .concat()
            .into(),
    ))
    .await
    .unwrap();
    let frame = next_kind(&mut ws, SYNCED).await;
    assert_eq!(&frame[1..6], &[42, 0, 0, 0, 0]);
    let error: Error = serde_json::from_slice(&frame[6..]).unwrap();
    assert_eq!(error.code, FAILED);
    assert!(error.message.contains("not a checkout"));
    ws.send(Message::Binary(vec![WATCH, 0].into()))
        .await
        .unwrap();
    drop(ws);
    node.stop().await;
    std::fs::remove_dir_all(home).unwrap();
}

async fn next_kind(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    kind: u8,
) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = ws.next().await {
            if let Message::Binary(bytes) = message.unwrap()
                && bytes.first() == Some(&kind)
            {
                return bytes.to_vec();
            }
        }
        panic!("socket closed before frame {kind:#x}")
    })
    .await
    .expect("timed out waiting for frame")
}
