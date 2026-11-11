//! Host commands, services routes and subscriptions through a testkit node.
use std::time::Duration;

use axum::http::Method;
use futures_util::{SinkExt, StreamExt};
use parvi_host_contract::{self as contract, ReadingFrame, ServiceList, State};
use parvi_node_kit::{NodeClient, cli};
use parvi_testkit::node::Node;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

async fn start() -> Node {
    Node::start(&[parvi_host::LISTING], "").await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn services_and_commands_use_the_same_catalog_and_coded_errors() {
    let node = start().await;
    let client = NodeClient::new(Some(&node.url())).unwrap();
    let missing = client
        .request(Method::GET, "/api/services", None, None)
        .await
        .unwrap_err();
    let missing = missing.downcast_ref::<parvi_node_kit::NodeError>().unwrap();
    assert_eq!(missing.code, contract::UNAVAILABLE);
    let missing = node.call("service_list", json!({})).await.unwrap_err();
    assert_eq!(missing.code, contract::UNAVAILABLE);
    assert!(missing.message.contains("No service catalog"));

    // A loopback fixture answers probes; no real service manager is touched.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = [0; 512];
                let _ = socket.read(&mut request).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    std::fs::write(
        node.dir().join("services.json"),
        json!({"services": [
            {"id":"fixture", "name":"Fixture", "readOnly":true,
             "components":[{"kind":"http", "url": format!("http://{addr}/")} ]},
            {"id":"failing", "name":"Failing", "components":[
                {"kind":"systemd-user", "user":"\u{0}", "unit":"example.service"}]}
        ]})
        .to_string(),
    )
    .unwrap();

    let (body, _) = client
        .request(Method::GET, "/api/services", None, None)
        .await
        .unwrap();
    let services: ServiceList = serde_json::from_value(body).unwrap();
    assert_eq!(services.services[0].id, "fixture");
    assert_eq!(services.services[0].state, "running");
    assert!(!services.services[0].controllable);
    let (one, _) = client
        .request(Method::GET, "/api/services/fixture", None, None)
        .await
        .unwrap();
    assert_eq!(one["service"]["state"], "running");
    let refused = client
        .request(Method::POST, "/api/services/fixture/restart", None, None)
        .await
        .unwrap_err();
    assert_eq!(
        refused
            .downcast_ref::<parvi_node_kit::NodeError>()
            .unwrap()
            .code,
        contract::BAD_REQUEST
    );
    let failed = client
        .request(Method::POST, "/api/services/failing/start", None, None)
        .await
        .unwrap_err();
    assert_eq!(
        failed
            .downcast_ref::<parvi_node_kit::NodeError>()
            .unwrap()
            .code,
        contract::SERVICE_FAILED
    );
    let not_found = client
        .request(Method::GET, "/api/services/absent", None, None)
        .await
        .unwrap_err();
    assert_eq!(
        not_found
            .downcast_ref::<parvi_node_kit::NodeError>()
            .unwrap()
            .code,
        contract::NOT_FOUND
    );

    let list = json!({"jsonrpc":"2.0", "id":1, "method":"tools/list"});
    let (body, _) = client
        .request(Method::POST, "/mcp", Some(list), None)
        .await
        .unwrap();
    let tools = body["result"]["tools"].as_array().unwrap();
    let names: Vec<_> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .filter(|name| !parvi_node_kit::own::COMMANDS.contains(name))
        .collect();
    assert_eq!(
        names,
        [
            "host_show",
            "service_list",
            "service_show",
            "service_start",
            "service_stop",
            "service_restart"
        ]
    );
    for tool in tools {
        assert_eq!(tool["inputSchema"]["properties"]["node"]["type"], "string");
    }
    let listed = node.call("service_list", json!({})).await.unwrap();
    assert_eq!(listed["services"][0]["state"], "running");
    let answer = node
        .call("service_show", json!({"id":"fixture"}))
        .await
        .unwrap();
    assert_eq!(answer["node"], node.id());
    assert_eq!(answer["service"]["state"], "running");
    assert_eq!(
        node.call("service_start", json!({"id":"fixture"}))
            .await
            .unwrap_err()
            .code,
        contract::BAD_REQUEST
    );
    assert_eq!(
        node.call("service_restart", json!({"id":"fixture"}))
            .await
            .unwrap_err()
            .code,
        contract::BAD_REQUEST
    );
    assert_eq!(
        node.call("service_show", json!({"id":42}))
            .await
            .unwrap_err()
            .code,
        contract::BAD_REQUEST
    );
    assert_eq!(
        node.call("service_start", json!({"id":"failing"}))
            .await
            .unwrap_err()
            .code,
        contract::SERVICE_FAILED
    );
    assert_eq!(
        node.call("service_list", json!({"node":"another-node"}))
            .await
            .unwrap_err()
            .code,
        "unknown_node"
    );
    let host = node.call("host_show", json!({})).await.unwrap();
    assert_eq!(host["node"], node.id());
    assert!(host["facts"]["threads"].as_u64().is_some_and(|n| n > 0));
    #[cfg(target_os = "linux")]
    assert!(host["latest"]["memory"].is_object());

    let host_args = ["show", "--node", &node.url(), "--json"].map(String::from);
    let mut host_output = Vec::new();
    assert_eq!(
        cli::run("host", &host_args, &mut host_output)
            .await
            .unwrap(),
        0
    );
    let cli_host: Value = serde_json::from_slice(&host_output).unwrap();
    assert!(cli_host["latest"].get("gpu_reason").is_some());

    let args = ["show", "fixture", "--node", &node.url(), "--json"].map(String::from);
    let mut output = Vec::new();
    assert_eq!(cli::run("service", &args, &mut output).await.unwrap(), 0);
    let cli_answer: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(cli_answer["service"]["id"], "fixture");
    server.abort();
    node.stop().await;
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn read(socket: &mut Socket) -> tokio_tungstenite::tungstenite::Bytes {
    tokio::time::timeout(Duration::from_secs(12), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_data()
}

#[tokio::test(flavor = "multi_thread")]
async fn socket_subscription_starts_with_state_streams_readings_and_stops() {
    let node = start().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(node.ws()).await.unwrap();
    let hello = socket.next().await.unwrap().unwrap();
    assert!(matches!(hello, Message::Binary(_)));
    socket
        .send(Message::Binary(contract::encode_on(true).into()))
        .await
        .unwrap();
    let state = read(&mut socket).await;
    assert_eq!(state[0], contract::HOST_STATE);
    let state: State = serde_json::from_slice(&state[1..]).unwrap();
    assert_eq!(state.every, 5000);
    let frame = read(&mut socket).await;
    assert_eq!(frame[0], contract::HOST_READING);
    let reading: ReadingFrame = serde_json::from_slice(&frame[1..]).unwrap();
    assert_eq!(reading.reading.at, reading.point.at);
    #[cfg(target_os = "linux")]
    assert!(reading.reading.memory.is_some());

    // A refused systemd invocation logs an event without reaching a manager.
    std::fs::write(
        node.dir().join("services.json"),
        json!({"services": [
            {"id":"failing", "name":"Failing", "components":[
                {"kind":"systemd-user", "user":"\u{0}", "unit":"example.service"}]}
        ]})
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        node.call("service_start", json!({"id":"failing"}))
            .await
            .unwrap_err()
            .code,
        contract::SERVICE_FAILED
    );
    let event = read(&mut socket).await;
    assert_eq!(event[0], contract::HOST_EVENT);
    let event: contract::EventFrame = serde_json::from_slice(&event[1..]).unwrap();
    assert_eq!(event.event.kind, "service");
    assert!(event.event.failure);
    socket
        .send(Message::Binary(contract::encode_on(false).into()))
        .await
        .unwrap();
    // The next sampling tick passes while the subscription is off.
    assert!(
        tokio::time::timeout(Duration::from_secs(7), socket.next())
            .await
            .is_err()
    );
    socket
        .send(Message::Binary(contract::encode_on(true).into()))
        .await
        .unwrap();
    assert_eq!(read(&mut socket).await[0], contract::HOST_STATE);
    drop(socket);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_host_has_no_routes_or_tools() {
    let node = Node::start(&[parvi_host::LISTING], "[host]\nenabled = false\n")
        .await
        .unwrap();
    let client = NodeClient::new(Some(&node.url())).unwrap();
    let (health, _) = client
        .request(Method::GET, "/api/health", None, None)
        .await
        .unwrap();
    assert_eq!(health["off"], json!(["host"]));
    assert!(
        client
            .request(Method::GET, "/api/services", None, None)
            .await
            .is_err()
    );
    let list = json!({"jsonrpc":"2.0", "id":1, "method":"tools/list"});
    let (body, _) = client
        .request(Method::POST, "/mcp", Some(list), None)
        .await
        .unwrap();
    let names: Vec<_> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, parvi_node_kit::own::COMMANDS, "only the node's own");
    node.stop().await;
}

#[cfg(unix)]
#[test]
fn host_cli_json_reports_gpu_failure() {
    use std::os::unix::fs::PermissionsExt;
    const CHILD: &str = "PARVI_GPU_REASON_CHILD";
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let node = start().await;
            let args = ["show", "--node", &node.url(), "--json"].map(String::from);
            let mut output = Vec::new();
            assert_eq!(cli::run("host", &args, &mut output).await.unwrap(), 0);
            let answer: Value = serde_json::from_slice(&output).unwrap();
            let reason = answer["latest"]["gpu_reason"].as_str().unwrap();
            assert!(
                reason.contains("9") && reason.contains("NVIDIA-SMI has failed"),
                "{reason}"
            );
            node.stop().await;
        });
        return;
    }
    let dir = parvi_testkit::fresh_dir("host-cli-gpu");
    let executable = dir.join("nvidia-smi");
    std::fs::write(
        &executable,
        "#!/bin/sh\necho 'NVIDIA-SMI has failed' >&2\nexit 9\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "host_cli_json_reports_gpu_failure",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("PATH", &dir)
        .status()
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(status.success());
}
