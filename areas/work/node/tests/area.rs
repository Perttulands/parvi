//! The area exposes the old Work commands and only pushes while watched.
use futures_util::{SinkExt, StreamExt};
use parvi_testkit::node::Node;
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[test]
fn commands_match_the_old_cli_and_mcp_tools() {
    assert_eq!(parvi_work::LISTING.commands, &["work_list", "work_show"]);
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
