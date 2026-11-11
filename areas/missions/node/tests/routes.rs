use axum::http::Method;
use parvi_node_kit::{Listing, NodeClient};
use parvi_testkit::{node::Node, sessions};
use serde_json::json;

const AREAS: &[Listing] = &[sessions::LISTING, parvi_missions_node::LISTING];

#[tokio::test(flavor = "multi_thread")]
async fn mission_routes_on_a_node_with_fake_sessions() {
    let node = Node::start(AREAS, "[missions]\ndir = \"missions\"\n")
        .await
        .unwrap();
    let client = NodeClient::new(Some(&format!("{}/", node.url()))).unwrap();
    let (created, etag) = client
        .request(
            Method::POST,
            "/api/missions",
            Some(json!({"title":"Example"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(created["mission"]["title"], "Example");
    let (listed, _) = client
        .request(Method::GET, "/api/missions", None, None)
        .await
        .unwrap();
    assert_eq!(listed["missions"][0]["slug"], "example");
    let (edited, _) = client
        .request(
            Method::PATCH,
            "/api/missions/example",
            Some(json!({"expectedRev":1,"title":"Renamed"})),
            Some(&etag),
        )
        .await
        .unwrap();
    assert_eq!(edited["mission"]["title"], "Renamed");
    let (layout, _) = client
        .request(Method::GET, "/api/missions/example/layout", None, None)
        .await
        .unwrap();
    assert!(layout["layout"]["nodes"].is_array());
    let error = client
        .request(Method::GET, "/api/missions/missing", None, None)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("not_found"));
    let other = NodeClient::new(Some(&format!("{}/?node=elsewhere", node.url()))).unwrap();
    let error = other
        .request(Method::GET, "/api/missions", None, None)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("unknown_node"));
    node.stop().await;
}
