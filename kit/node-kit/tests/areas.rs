//! An area defined here, in the test, reaches the client every way the node
//! offers: its route over HTTP, its channel on `/ws` (a frame from the
//! client and a push), and its command as an MCP tool and a CLI verb. And
//! areas that would share a name, a frame kind or a command refuse to start.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use axum::Router;
use axum::http::Method;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use parvi_core::Error;
use parvi_core::Hello;
use parvi_core::frame::Kinds;
use parvi_node_kit::{
    Area, BoxFuture, Build, Channel, Client, Command, Config, NodeClient, Options, Running, Setup,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// A lamp: `/api/lights` says how bright, `lights_switch` sets it, and a
/// client's frame 0x60 asks for a push 0x61 of the brightness it carries.
struct Lights {
    name: &'static str,
    kinds: Kinds,
    command: &'static str,
}

const LIGHTS: Kinds = &[0x60..=0x61];

impl Area for Lights {
    fn name(&self) -> &'static str {
        self.name
    }

    fn kinds(&self) -> Kinds {
        self.kinds
    }

    fn routes(&self) -> Router {
        Router::new().route(
            "/api/lights",
            get(|| async { axum::Json(json!({ "brightness": "dim" })) }),
        )
    }

    fn commands(&self) -> Vec<Command> {
        vec![Command::new(
            self.command,
            "Switch the lights. Answers with how bright they are.",
            || json!({ "type": "object", "properties": { "level": { "type": "integer" } } }),
            |args: Value| async move {
                match args["level"].as_u64() {
                    Some(level) => Ok(json!({ "level": level })),
                    None => Err(Error::new("no_level", "level is required")),
                }
            },
        )]
    }

    fn channel(&self, _client: Client) -> Option<Box<dyn Channel>> {
        let (pushes, pushed) = mpsc::channel(4);
        Some(Box::new(Lamp { pushes, pushed }))
    }
}

struct Lamp {
    pushes: mpsc::Sender<Vec<u8>>,
    pushed: mpsc::Receiver<Vec<u8>>,
}

impl Channel for Lamp {
    fn receive(&mut self, frame: Vec<u8>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            assert_eq!(frame[0], 0x60);
            let _ = self.pushes.send(vec![0x61, frame[1]]).await;
        })
    }

    fn next(&mut self) -> BoxFuture<'_, Option<Vec<u8>>> {
        Box::pin(self.pushed.recv())
    }
}

fn lights(_: &Setup) -> anyhow::Result<Arc<dyn Area>> {
    Ok(Arc::new(Lights {
        name: "lights",
        kinds: LIGHTS,
        command: "lights_switch",
    }))
}

async fn start(areas: &[(&'static str, Build)], config: &str) -> anyhow::Result<Running> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "parvi-areas-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir)?;
    let config = Config::parse(config, dir.clone())?;
    parvi_node_kit::start(
        Options {
            dir,
            config,
            listen: Some("127.0.0.1:0".into()),
        },
        areas,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_area_defined_in_a_test_answers_every_way() {
    let node = start(&[("lights", lights)], "").await.unwrap();
    let url = format!("http://{}/", node.address);
    let client = NodeClient::new(Some(&url)).unwrap();

    // Its route, over HTTP.
    let (dim, _) = client
        .request(Method::GET, "/api/lights", None, None)
        .await
        .unwrap();
    assert_eq!(dim, json!({ "brightness": "dim" }));

    // Its channel, on /ws: after the hello, a frame and its push.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", node.address))
        .await
        .unwrap();
    let hello = ws.next().await.unwrap().unwrap().into_data();
    let hello = Hello::decode(&hello).unwrap();
    assert_eq!(hello.node, node.node.id.to_string());
    assert!(hello.off.is_empty());
    ws.send(Message::Binary(vec![0x60, 7].into()))
        .await
        .unwrap();
    let pushed = ws.next().await.unwrap().unwrap().into_data();
    assert_eq!(&pushed[..], &[0x61, 7]);

    // Its command, as an MCP tool.
    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "lights_switch", "arguments": { "level": 3 } },
    });
    let (answer, _) = client
        .request(Method::POST, "/mcp", Some(call), None)
        .await
        .unwrap();
    assert_eq!(answer["result"]["isError"], false);
    assert_eq!(answer["result"]["content"][0]["text"], r#"{"level":3}"#);
    let failing = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "lights_switch", "arguments": {} },
    });
    let (answer, _) = client
        .request(Method::POST, "/mcp", Some(failing), None)
        .await
        .unwrap();
    assert_eq!(answer["result"]["isError"], true);
    let error: Error =
        serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error.code, "no_level");
    // `node` names where a tool runs: this node, and no other yet.
    for (node, code) in [
        (node.node.id.to_string(), None),
        ("elsewhere".into(), Some("unknown_node")),
    ] {
        let call = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "lights_switch", "arguments": { "level": 1, "node": node } },
        });
        let (answer, _) = client
            .request(Method::POST, "/mcp", Some(call), None)
            .await
            .unwrap();
        let text = answer["result"]["content"][0]["text"].as_str().unwrap();
        match code {
            None => assert_eq!(text, r#"{"level":1}"#),
            Some(code) => {
                let error: Error = serde_json::from_str(text).unwrap();
                assert_eq!(error.code, code);
            }
        }
    }

    // And as a CLI verb.
    let mut out = Vec::new();
    let args: Vec<String> = ["switch", "--level", "5", "--json", "--node", &url]
        .map(String::from)
        .to_vec();
    let code = parvi_node_kit::cli::run("lights", &args, &mut out)
        .await
        .unwrap();
    assert_eq!(code, 0);
    let printed: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(printed, json!({ "level": 5 }));

    drop(ws);
    node.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_area_its_section_turns_off_is_not_built_and_says_so() {
    let node = start(&[("lights", lights)], "[lights]\nenabled = false\n")
        .await
        .unwrap();
    assert_eq!(node.off(), ["lights"]);
    let url = format!("http://{}/", node.address);
    let client = NodeClient::new(Some(&url)).unwrap();
    let (health, _) = client
        .request(Method::GET, "/api/health", None, None)
        .await
        .unwrap();
    assert_eq!(health["off"], json!(["lights"]));
    let args: Vec<String> = ["switch", "--node", &url].map(String::from).to_vec();
    let refused = parvi_node_kit::cli::run("lights", &args, &mut Vec::new())
        .await
        .unwrap_err();
    assert!(
        format!("{refused:#}").contains("turned off on this node: lights"),
        "{refused:#}"
    );
    node.stop().await.unwrap();
}

/// Starts `first` and `second` together and answers with the refusal.
async fn refusal(first: Build, second: Build) -> String {
    match start(&[("lights", first), ("lamps", second)], "").await {
        Ok(_) => panic!("conflicting areas started"),
        Err(error) => format!("{error:#}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn areas_that_would_share_a_kind_a_name_or_a_command_refuse_to_start() {
    fn lamps_sharing_a_kind(_: &Setup) -> anyhow::Result<Arc<dyn Area>> {
        Ok(Arc::new(Lights {
            name: "lamps",
            kinds: &[0x61..=0x62],
            command: "lamps_switch",
        }))
    }
    let error = refusal(lights, lamps_sharing_a_kind).await;
    assert!(
        error.contains("lights") && error.contains("lamps") && error.contains("frame kinds"),
        "{error}"
    );

    fn lamps_sharing_a_command(_: &Setup) -> anyhow::Result<Arc<dyn Area>> {
        Ok(Arc::new(Lights {
            name: "lamps",
            kinds: &[0x70..=0x70],
            command: "lights_switch",
        }))
    }
    let error = refusal(lights, lamps_sharing_a_command).await;
    assert!(
        error.contains("lights") && error.contains("lamps") && error.contains("lights_switch"),
        "{error}"
    );

    fn lamps_named_lights(_: &Setup) -> anyhow::Result<Arc<dyn Area>> {
        Ok(Arc::new(Lights {
            name: "lights",
            kinds: &[0x70..=0x70],
            command: "lamps_switch",
        }))
    }
    // Listed as lamps, it calls itself lights.
    let error = refusal(lights, lamps_named_lights).await;
    assert!(
        error.contains("lights") && error.contains("lamps"),
        "{error}"
    );
    // Listed twice under one name.
    let error = match start(&[("lights", lights), ("lights", lights)], "").await {
        Ok(_) => panic!("two areas named lights started"),
        Err(error) => format!("{error:#}"),
    };
    assert!(error.contains("two areas are named lights"), "{error}");

    fn hello_taker(_: &Setup) -> anyhow::Result<Arc<dyn Area>> {
        Ok(Arc::new(Lights {
            name: "lamps",
            kinds: &[0x00..=0x00],
            command: "lamps_switch",
        }))
    }
    let error = refusal(lights, hello_taker).await;
    assert!(
        error.contains("lamps") && error.contains("hello"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_section_no_area_reads_refuses_to_start() {
    let error = match start(&[("lights", lights)], "[lamps]\nlevel = 3\n").await {
        Ok(_) => panic!("an unknown section started"),
        Err(error) => format!("{error:#}"),
    };
    assert!(error.contains("lamps"), "{error}");
}

const UPGRADE: &str = "Connection: Upgrade\r\nUpgrade: websocket\r\n\
    Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n";

/// Every way in, the node's own and an area's, admits only requests
/// addressed to this node from its own pages or from no page at all. The
/// HTTPS rule for tailnet names is `serve::trust`'s own test: no node learns
/// tailnet names yet.
#[tokio::test(flavor = "multi_thread")]
async fn foreign_origins_and_rebinding_hosts_are_refused() {
    let node = start(&[("lights", lights)], "").await.unwrap();
    let here = node.address.to_string();
    let own = format!("http://{here}");
    let cases = [
        (here.as_str(), None, true, "without Origin"),
        (&here, Some(own.as_str()), true, "from its own origin"),
        (
            &here,
            Some("http://evil.example"),
            false,
            "from a foreign origin",
        ),
        (
            &here,
            Some("http://localhost:1"),
            false,
            "from another port",
        ),
        ("evil.example:7300", None, false, "rebinding without Origin"),
        (
            "evil.example:7300",
            Some("http://evil.example:7300"),
            false,
            "rebinding with Origin",
        ),
    ];
    for (path, upgrade, served) in [
        ("/api/health", "", 200),
        ("/ws", UPGRADE, 101),
        ("/mcp", "", 405),
        ("/api/lights", "", 200),
    ] {
        for (host, origin, admitted, case) in cases {
            let expected = if admitted { served } else { 403 };
            let got = status(node.address, path, upgrade, host, origin).await;
            assert_eq!(got, expected, "{path} {case}");
        }
    }
    node.stop().await.unwrap();
}

/// The status a request for `path` gets with these headers.
async fn status(
    addr: std::net::SocketAddr,
    path: &str,
    upgrade: &str,
    host: &str,
    origin: Option<&str>,
) -> u16 {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let origin = origin
        .map(|o| format!("Origin: {o}\r\n"))
        .unwrap_or_default();
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{origin}{upgrade}\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).await.unwrap();
    line.split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("status line {line:?}"))
}
