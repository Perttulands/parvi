//! The whole binary: `parvi serve` holding only terminals over a scratch
//! tmux, the CLI over its commands, and `parvi mcp` relaying stdio. Linux
//! and macOS only; skips without tmux unless `PARVI_REQUIRE_TMUX=1`.
#![cfg(unix)]

use std::net::SocketAddr;
use std::process::{Output, Stdio};
use std::time::Duration;

use parvi_testkit::process::Node;
use parvi_testkit::tmux::{Server, capture, eventually};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const PARVI: &str = env!("CARGO_BIN_EXE_parvi");

/// A `parvi serve` whose only section is terminals on `tmux`'s server,
/// with its config folder in `tmux`'s directory.
fn serve(tmux: &Server) -> Node {
    let config = tmux.dir().join("config.toml");
    std::fs::write(
        &config,
        format!("[terminals]\nsocket = {:?}\n", tmux.socket_path()),
    )
    .unwrap();
    let mut command = std::process::Command::new(PARVI);
    command
        .args(["serve", "--config"])
        .arg(&config)
        .env("XDG_CONFIG_HOME", tmux.dir().join("xdg"));
    Node::start(&mut command, "127.0.0.1:0".parse().unwrap())
}

async fn parvi(node: SocketAddr, args: &[&str]) -> Output {
    tokio::process::Command::new(PARVI)
        .args(args)
        .args(["--node", &format!("http://{node}")])
        .output()
        .await
        .unwrap()
}

async fn get(node: SocketAddr, path: &str) -> Value {
    let mut stream = tokio::net::TcpStream::connect(node).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: {node}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{response}");
    serde_json::from_str(body).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn serve_with_only_terminals_answers_health_and_the_session_commands() {
    let Some(tmux) = Server::start_in("parvi-serve") else {
        return;
    };
    tmux.new_session("alpha", 100, 30);
    let node = serve(&tmux);
    let health = get(node.address, "/api/health").await;
    assert_eq!(health["status"], "ok", "{health}");

    let sent = parvi(
        node.address,
        &["session", "send", "alpha", "echo cli-$((6*7))"],
    )
    .await;
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    eventually("the echo ran", || {
        capture(&tmux, "alpha")
            .lines()
            .any(|line| line.trim() == "cli-42")
            .then_some(())
    })
    .await;
    let listed = parvi(node.address, &["session", "list", "--json"]).await;
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let sessions: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert!(sessions.to_string().contains("\"alpha\""), "{sessions}");
    let verbs = parvi(node.address, &["session"]).await;
    let verbs = String::from_utf8_lossy(&verbs.stdout);
    assert!(
        verbs.contains("parvi session send <session> <text>"),
        "{verbs}"
    );
    let missing = parvi(node.address, &["session", "send", "alpha"]).await;
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("missing <text>"));
    let unknown = parvi(node.address, &["session", "read", "nobody-here"]).await;
    assert_eq!(unknown.status.code(), Some(1));
}

/// Linux only: the environment it reports is Linux or WSL.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn health_and_version_identify_the_serving_node() {
    let root = parvi_testkit::fresh_dir("parvi-health");
    let config = root.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[node]\ndevice = \"health-test\"\n[terminals]\nsocket = {:?}\n",
            root.join("tmux")
        ),
    )
    .unwrap();
    let mut command = std::process::Command::new(PARVI);
    command
        .args(["serve", "--config"])
        .arg(&config)
        .env("XDG_CONFIG_HOME", root.join("xdg"));
    let mut node = Node::start(&mut command, "127.0.0.1:0".parse().unwrap());
    let health = get(node.address, "/api/health").await;
    let version = get(node.address, "/api/version").await;
    assert_eq!(health["status"], "ok");
    assert_eq!(health["device"], "health-test");
    assert!(!health["node"].as_str().unwrap().is_empty());
    assert!(matches!(
        health["environment"].as_str(),
        Some("linux" | "wsl")
    ));
    assert_eq!(health["protocol"], parvi_node_kit::serve::PROTOCOL);
    assert_eq!(health["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(version["version"], health["version"]);
    assert!(
        health["timestamp"]
            .as_str()
            .unwrap()
            .parse::<jiff::Timestamp>()
            .is_ok()
    );
    node.stop();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn parvi_mcp_relays_stdio_to_the_node() {
    let Some(tmux) = Server::start_in("parvi-stdio") else {
        return;
    };
    tmux.new_session("alpha", 100, 30);
    let node = serve(&tmux);
    let mut child = tokio::process::Command::new(PARVI)
        .args(["mcp", "--node", &node.address.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let messages = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-03-26", "capabilities": {},
                            "clientInfo": { "name": "test", "version": "1" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "session_list", "arguments": {} } }),
    ];
    for message in messages {
        stdin
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }
    let next = async |lines: &mut tokio::io::Lines<_>| -> Value {
        let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let answer = next(&mut stdout).await;
    assert_eq!(answer["result"]["protocolVersion"], "2025-03-26");
    let answer = next(&mut stdout).await;
    assert_eq!(answer["id"], 2, "the notification got no answer line");
    let sessions: Value =
        serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(
        sessions["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "alpha"),
        "{sessions}"
    );

    drop(stdin);
    let status = child.wait().await.unwrap();
    assert!(status.success(), "ends with stdin");
}

#[tokio::test(flavor = "multi_thread")]
async fn parvi_mcp_answers_requests_when_the_node_is_down() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let mut child = tokio::process::Command::new(PARVI)
        .args(["mcp", "--node", &addr.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n")
        .await
        .unwrap();
    drop(stdin);
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .await
        .unwrap();
    let answer: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(answer["id"], 7);
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("parvi node at")
    );
}
