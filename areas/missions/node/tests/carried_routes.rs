//! Route regressions carried from the former Missions node.
use axum::http::{Method, StatusCode};
use parvi_node_kit::{NodeClient, NodeError};
use parvi_testkit::{node::Node, sessions};
use serde_json::{Value, json};

async fn start() -> (Node, NodeClient) {
    let node = Node::start(
        &[
            sessions::LISTING,
            parvi_work::LISTING,
            parvi_missions::LISTING,
        ],
        "",
    )
    .await
    .unwrap();
    let client = NodeClient::new(Some(&format!("{}/", node.url()))).unwrap();
    (node, client)
}
async fn send(
    client: &NodeClient,
    method: Method,
    path: &str,
    body: Value,
    etag: Option<&str>,
) -> (Value, String) {
    client
        .request(method, path, Some(body), etag)
        .await
        .unwrap()
}
fn assert_error(error: anyhow::Error, status: StatusCode, code: &str) {
    let error = error.downcast_ref::<NodeError>().unwrap();
    assert_eq!((error.status, error.code.as_str()), (status, code));
}

#[tokio::test(flavor = "multi_thread")]
async fn http_restores_a_deleted_formation_and_fences_notes_and_layout() {
    let (node, c) = start().await;
    let (_, tag) = send(
        &c,
        Method::POST,
        "/api/missions",
        json!({"slug":"demo","title":"Demo"}),
        None,
    )
    .await;
    let (made, tag) = send(
        &c,
        Method::PATCH,
        "/api/missions/demo",
        json!({"expectedRev":1,"createFormation":{"type":"solo","title":"Work"}}),
        Some(&tag),
    )
    .await;
    let formation = made["formation"].clone();
    let id = formation["id"].as_str().unwrap();
    let (deleted, tag) = send(
        &c,
        Method::PATCH,
        "/api/missions/demo",
        json!({"expectedRev":2,"deleteFormation":{"id":id}}),
        Some(&tag),
    )
    .await;
    assert_eq!(deleted["mission"]["formations"], json!([]));
    let (restored, _) = send(
        &c,
        Method::PATCH,
        "/api/missions/demo",
        json!({"expectedRev":3,"restoreNode":{"formation":formation,"x":112,"y":112}}),
        Some(&tag),
    )
    .await;
    assert_eq!(restored["mission"]["formations"][0]["id"], id);
    let (_, notes_tag) = c
        .request(Method::GET, "/api/missions/demo/notes", None, None)
        .await
        .unwrap();
    let note = json!({"target":"mission","author":"human:test","text":"Keep this"});
    send(
        &c,
        Method::PATCH,
        "/api/missions/demo/notes",
        note.clone(),
        Some(&notes_tag),
    )
    .await;
    assert_error(
        c.request(
            Method::PATCH,
            "/api/missions/demo/notes",
            Some(note),
            Some(&notes_tag),
        )
        .await
        .unwrap_err(),
        StatusCode::CONFLICT,
        "conflict",
    );
    let (_, layout_tag) = c
        .request(Method::GET, "/api/missions/demo/layout", None, None)
        .await
        .unwrap();
    let position = json!({"nodes":[{"id":id,"x":400,"y":500}]});
    send(
        &c,
        Method::PATCH,
        "/api/missions/demo/layout",
        position.clone(),
        Some(&layout_tag),
    )
    .await;
    assert_error(
        c.request(
            Method::PATCH,
            "/api/missions/demo/layout",
            Some(position),
            Some(&layout_tag),
        )
        .await
        .unwrap_err(),
        StatusCode::CONFLICT,
        "conflict",
    );
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn derives_named_slug_and_rejects_duplicate_without_clobbering() {
    let (node, c) = start().await;
    let body = json!({"title":"  Release Plan!  "});
    let (first, tag) = send(&c, Method::POST, "/api/missions", body.clone(), None).await;
    assert_eq!(first["mission"]["slug"], "release-plan");
    assert_eq!(first["mission"]["title"], "Release Plan!");
    assert_eq!(first["mission"]["etag"], tag);
    assert_error(
        c.request(Method::POST, "/api/missions", Some(body), None)
            .await
            .unwrap_err(),
        StatusCode::CONFLICT,
        "mission_exists",
    );
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn default_gate_is_human_and_a_code_check_requires_code_kind() {
    let (node, c) = start().await;
    let (_, tag) = send(
        &c,
        Method::POST,
        "/api/missions",
        json!({"slug":"gate","title":"Gate"}),
        None,
    )
    .await;
    let bad=c.request(Method::PATCH,"/api/missions/gate",Some(json!({"expectedRev":1,"createGate":{"check":"output_contains","checkVersion":"1","checkValue":"OK"}})),Some(&tag)).await.unwrap_err();
    let err = bad.downcast_ref::<NodeError>().unwrap();
    assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    let (saved, _) = send(
        &c,
        Method::PATCH,
        "/api/missions/gate",
        json!({"expectedRev":1,"createGate":{"title":"Signoff"}}),
        Some(&tag),
    )
    .await;
    assert_eq!(saved["gate"]["kinds"], json!(["human"]));
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_character_has_the_documented_error_envelope() {
    let (node, c) = start().await;
    let bad = c
        .request(
            Method::POST,
            "/api/characters",
            Some(json!({"name":"A","extra":true})),
            None,
        )
        .await
        .unwrap_err();
    assert_error(bad, StatusCode::UNPROCESSABLE_ENTITY, "invalid_character");
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_are_checked_and_pushed_to_subscribers() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let (node, c) = start().await;
    let (mut socket, _) = connect_async(node.ws()).await.unwrap();
    socket
        .send(Message::Binary(
            vec![parvi_missions_contract::WATCH, 1].into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Binary(frame) = message
                && frame.first() == Some(&parvi_missions_contract::SNAPSHOT)
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    let (_, tag) = send(
        &c,
        Method::POST,
        "/api/missions",
        json!({"slug":"test","title":"Test"}),
        None,
    )
    .await;
    let (_, same) = send(
        &c,
        Method::PATCH,
        "/api/missions/test",
        json!({"expectedRev":1,"title":"Test"}),
        Some(&tag),
    )
    .await;
    assert_eq!(same, tag);
    let error = c
        .request(
            Method::PATCH,
            "/api/missions/test",
            Some(json!({"expectedRev":1,"title":"X","createEnd":{"title":"End"}})),
            Some(&tag),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<NodeError>().unwrap().status,
        StatusCode::BAD_REQUEST
    );
    let update = json!({"expectedRev":1,"title":"Renamed"});
    send(
        &c,
        Method::PATCH,
        "/api/missions/test",
        update.clone(),
        Some(&tag),
    )
    .await;
    let stale = c
        .request(
            Method::PATCH,
            "/api/missions/test",
            Some(update),
            Some(&tag),
        )
        .await
        .unwrap_err();
    assert_eq!(
        stale.downcast_ref::<NodeError>().unwrap().status,
        StatusCode::CONFLICT
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Binary(frame) = message
                && frame.first() == Some(&parvi_missions_contract::SNAPSHOT)
            {
                let data: Value = serde_json::from_slice(&frame[1..]).unwrap();
                if data.to_string().contains("Renamed") {
                    return;
                }
            }
        }
        panic!("subscription ended before the edit");
    })
    .await
    .expect("subscriber observes edit");
    drop(socket);
    node.stop().await;
}
