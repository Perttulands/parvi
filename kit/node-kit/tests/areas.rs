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
    Area, BoxFuture, Build, Built, Channel, Client, Command, Config, Listing, NodeClient,
    NodeError, Options, Running, Setup,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// A lamp: `/api/lights` says how bright, `lights_switch` sets it, and a
/// client's frame 0x60 asks for a push 0x61 of the brightness it carries.
struct Lights {
    command: &'static str,
}

const LIGHTS: Kinds = &[0x60..=0x61];

/// The lights as a binary lists them.
const LIGHTS_LISTED: Listing = listed("lights", LIGHTS, &["lights_switch"], lights);

const fn listed(
    name: &'static str,
    kinds: Kinds,
    commands: &'static [&'static str],
    build: Build,
) -> Listing {
    Listing {
        name,
        kinds,
        commands,
        build,
    }
}

impl Area for Lights {
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

fn lights(_: &Setup) -> anyhow::Result<Built> {
    Built::now(Lights {
        command: "lights_switch",
    })
}

async fn start(areas: &[Listing], config: &str) -> anyhow::Result<Running> {
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
    let node = start(&[LIGHTS_LISTED], "").await.unwrap();
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
    let node = start(&[LIGHTS_LISTED], "[lights]\nenabled = false\n")
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

/// Starts the lights and `second` together and answers with the refusal.
async fn refusal(second: Listing) -> String {
    match start(&[LIGHTS_LISTED, second], "").await {
        Ok(_) => panic!("conflicting areas started"),
        Err(error) => format!("{error:#}"),
    }
}

/// The node checks its list before building anything (`listing::check`
/// holds each conflict), and refuses an area whose built commands are not
/// the ones it is listed with.
#[tokio::test(flavor = "multi_thread")]
async fn areas_that_would_share_a_kind_a_name_or_a_command_refuse_to_start() {
    let error = refusal(listed("lamps", &[0x61..=0x62], &["lamps_switch"], lights)).await;
    assert!(
        error.contains("lights") && error.contains("lamps") && error.contains("frame kinds"),
        "{error}"
    );
    let error = refusal(listed("lamps", &[0x70..=0x70], &["lights_switch"], lights)).await;
    assert!(
        error.contains("lights") && error.contains("lamps") && error.contains("lights_switch"),
        "{error}"
    );
    let error = refusal(LIGHTS_LISTED).await;
    assert!(error.contains("two areas are named lights"), "{error}");
    let error = refusal(listed("lamps", &[0x00..=0x00], &[], lights)).await;
    assert!(
        error.contains("lamps") && error.contains("hello"),
        "{error}"
    );

    fn lamps(_: &Setup) -> anyhow::Result<Built> {
        Built::now(Lights {
            command: "lamps_dim",
        })
    }
    // Listed with one command, it has another.
    let error = refusal(listed("lamps", &[0x70..=0x70], &["lamps_switch"], lamps)).await;
    assert!(
        error.contains("lamps") && error.contains("lamps_switch") && error.contains("lamps_dim"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_section_no_area_reads_refuses_to_start() {
    let error = match start(&[LIGHTS_LISTED], "[lamps]\nlevel = 3\n").await {
        Ok(_) => panic!("an unknown section started"),
        Err(error) => format!("{error:#}"),
    };
    assert!(error.contains("lamps"), "{error}");
}

/// What one area offers others: a word to say.
trait Word: Send + Sync {
    fn word(&self) -> &'static str;
}

impl Word for Lights {
    fn word(&self) -> &'static str {
        "bright"
    }
}

/// Lights that offer their word to other areas.
fn offering(_: &Setup) -> anyhow::Result<Built> {
    Ok(Built::Now(Arc::new(Lights {
        command: "lights_switch",
    })))
}

fn offering_word(setup: &Setup) -> anyhow::Result<Built> {
    setup.provided.provide::<dyn Word>(Arc::new(Lights {
        command: "lights_switch",
    }))?;
    offering(setup)
}

/// Lamps that read the word lights offer, and answer it as their command.
fn reading_word(_: &Setup) -> anyhow::Result<Built> {
    Built::then(|provided| {
        let word = provided.get::<dyn Word>()?.word();
        Ok(Arc::new(Said { word }) as Arc<dyn Area>)
    })
}

const LIGHTS_OFFERING: Listing = listed("lights", LIGHTS, &["lights_switch"], offering_word);
const LAMPS_READING: Listing = listed("lamps", &[], &["lamps_say"], reading_word);

struct Said {
    word: &'static str,
}

impl Area for Said {
    fn commands(&self) -> Vec<Command> {
        let word = self.word;
        vec![Command::new(
            "lamps_say",
            "Says the word the lights offer.",
            || json!({ "type": "object" }),
            move |_| async move { Ok(json!(word)) },
        )]
    }
}

/// An area reads what another provides wherever the two are listed; read
/// while areas are still being built it is refused, and with no provider
/// it is `not_provided`.
#[tokio::test(flavor = "multi_thread")]
async fn areas_read_what_others_provide_in_any_order() {
    for areas in [
        [LAMPS_READING, LIGHTS_OFFERING],
        [LIGHTS_OFFERING, LAMPS_READING],
    ] {
        let node = start(&areas, "").await.unwrap();
        let said = node.provided.get::<dyn Word>().unwrap().word();
        assert_eq!(said, "bright");
        let url = format!("http://{}/", node.address);
        let call = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "lamps_say", "arguments": {} },
        });
        let (answer, _) = NodeClient::new(Some(&url))
            .unwrap()
            .request(Method::POST, "/mcp", Some(call), None)
            .await
            .unwrap();
        assert_eq!(answer["result"]["content"][0]["text"], r#""bright""#);
        node.stop().await.unwrap();
    }

    let error = match start(&[LAMPS_READING, LIGHTS_LISTED], "").await {
        Ok(_) => panic!("an area read what nobody provides"),
        Err(error) => format!("{error:#}"),
    };
    assert!(error.contains("starting lamps"), "{error}");
    assert!(error.contains("no area on this node provides"), "{error}");

    fn reading_early(setup: &Setup) -> anyhow::Result<Built> {
        setup.provided.get::<dyn Word>()?;
        offering(setup)
    }
    let error = match start(
        &[
            LIGHTS_OFFERING,
            listed("lamps", &[], &["lamps_switch"], reading_early),
        ],
        "",
    )
    .await
    {
        Ok(_) => panic!("an area read a slot while areas were being built"),
        Err(error) => format!("{error:#}"),
    };
    assert!(error.contains("Built::then"), "{error}");
}

const UPGRADE: &str = "Connection: Upgrade\r\nUpgrade: websocket\r\n\
    Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n";

/// Every way in, the node's own and an area's, admits only requests
/// addressed to this node from its own pages or from no page at all. The
/// HTTPS rule for tailnet names is `serve::trust`'s own test: no node learns
/// tailnet names yet.
#[tokio::test(flavor = "multi_thread")]
async fn foreign_origins_and_rebinding_hosts_are_refused() {
    let node = start(&[LIGHTS_LISTED], "").await.unwrap();
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

/// A request naming another node with `?node=` is refused with the error
/// shape, on an area's route, on `/ws` and on `/mcp`, and never runs here;
/// one naming this node runs as one naming none.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_for_another_node_is_refused_not_run_here() {
    let node = start(&[LIGHTS_LISTED], "").await.unwrap();
    let here = node.address.to_string();
    let own = node.node.id.to_string();
    for (path, upgrade, served) in [
        ("/api/lights", "", 200),
        ("/ws", UPGRADE, 101),
        ("/mcp", "", 405),
    ] {
        let mine = format!("{path}?node={own}");
        assert_eq!(
            status(node.address, &mine, upgrade, &here, None).await,
            served
        );
        let theirs = format!("{path}?node=elsewhere");
        let (code, body) = response(node.address, &theirs, upgrade).await;
        assert_eq!(code, 421, "{theirs}");
        let refused: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(refused["error"]["code"], "unknown_node", "{theirs}");
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap()
                .contains("elsewhere")
        );
    }
    node.stop().await.unwrap();
}

/// The status and body a request for `path` gets.
async fn response(addr: std::net::SocketAddr, path: &str, upgrade: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n{upgrade}\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut text = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await.unwrap();
        text.extend_from_slice(&chunk[..read]);
        let whole = String::from_utf8_lossy(&text).into_owned();
        if let Some((head, body)) = whole.split_once("\r\n\r\n") {
            let length: usize = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::to_owned)
                })
                .and_then(|n| n.trim().parse().ok())
                .unwrap_or(0);
            if body.len() >= length || read == 0 {
                let code = head.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap();
                return (code, body.to_owned());
            }
        }
        assert!(read > 0, "the node closed before answering: {whole}");
    }
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

/// An area that reads its section and does nothing else.
fn files(_: &Setup) -> anyhow::Result<Built> {
    struct Files;
    impl Area for Files {}
    Built::now(Files)
}

/// `parvi <noun> <args>` against the node at `url`: its exit code and what
/// it printed.
async fn parvi(noun: &str, args: &[&str], url: &str) -> (i32, Value) {
    let mut args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    args.extend(["--json".into(), "--node".into(), url.into()]);
    let mut out = Vec::new();
    let code = parvi_node_kit::cli::run(noun, &args, &mut out)
        .await
        .unwrap();
    (code, serde_json::from_slice(&out).unwrap_or(Value::Null))
}

/// The node's own command and route show its config file as it loaded it.
#[tokio::test(flavor = "multi_thread")]
async fn config_show_answers_the_file_and_each_section() {
    let node = start(
        &[listed("files", &[], &[], files), LIGHTS_LISTED],
        "[files]\nroots = [\"/srv/notes\"]\n[lights]\nenabled = true\n",
    )
    .await
    .unwrap();
    let url = format!("http://{}/", node.address);
    let (code, all) = parvi("config", &["show"], &url).await;
    assert_eq!(code, 0);
    let path = all["path"].as_str().unwrap();
    assert!(path.ends_with("config.toml"), "{path}");
    assert_eq!(all["sections"]["files"]["roots"], json!(["/srv/notes"]));
    assert_eq!(all["sections"]["lights"]["enabled"], json!(true));

    let (code, files) = parvi("config", &["show", "--section", "files"], &url).await;
    assert_eq!(code, 0);
    assert_eq!(
        files["sections"],
        json!({"files": {"roots": ["/srv/notes"]}})
    );
    let (code, _) = parvi("config", &["show", "--section", "nope"], &url).await;
    assert_eq!(code, 1);

    let client = NodeClient::new(Some(&url)).unwrap();
    let (answer, _) = client
        .request(Method::GET, "/api/config?section=files", None, None)
        .await
        .unwrap();
    assert_eq!(answer, files);
    let refused = client
        .request(Method::GET, "/api/config?section=nope", None, None)
        .await
        .unwrap_err();
    assert_eq!(
        refused.downcast_ref::<NodeError>().unwrap().code,
        "unknown_section"
    );

    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let (tools, _) = client
        .request(Method::POST, "/mcp", Some(list), None)
        .await
        .unwrap();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for own in ["config_show", "view_show", "view_open"] {
        assert!(names.contains(&own), "{names:?}");
    }
    node.stop().await.unwrap();
}

/// An MCP tool call's answer: its value, or its error's code.
async fn tool(client: &NodeClient, name: &str, arguments: Value) -> Result<Value, String> {
    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let (answer, _) = client
        .request(Method::POST, "/mcp", Some(call), None)
        .await
        .unwrap();
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    match answer["result"]["isError"].as_bool() {
        Some(false) => Ok(serde_json::from_str(text).unwrap()),
        _ => Err(serde_json::from_str::<Error>(text).unwrap().code),
    }
}

/// View control, the node's side, with a socket client standing in for the
/// app: it is an app once it says what it shows, and `view_open` reaches it
/// as a view command whose answer is the command's.
#[tokio::test(flavor = "multi_thread")]
async fn view_open_reaches_the_app_and_answers_its_answer() {
    let node = start(&[], "").await.unwrap();
    let url = format!("http://{}/", node.address);
    let client = NodeClient::new(Some(&url)).unwrap();
    assert_eq!(parvi("view", &["show"], &url).await, (0, json!([])));
    assert_eq!(
        parvi("view", &["open", "--page", "files"], &url).await.0,
        1,
        "no app"
    );
    let args = json!({ "page": "files" });
    assert_eq!(
        tool(&client, "view_open", args).await.unwrap_err(),
        "no_app"
    );

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", node.address))
        .await
        .unwrap();
    ws.next().await.unwrap().unwrap(); // the hello
    // Not an app until it says what it shows.
    assert_eq!(tool(&client, "view_show", json!({})).await, Ok(json!([])));
    ws.send(Message::Binary(
        [&[0xb0][..], br#"{"page":"terminals"}"#].concat().into(),
    ))
    .await
    .unwrap();
    let shown = loop {
        let shown = tool(&client, "view_show", json!({})).await.unwrap();
        if shown != json!([]) {
            break shown;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert_eq!(shown[0]["page"], "terminals");

    // Two things at once are refused before reaching the app.
    let two = json!({ "page": "files", "path": "/a", "item": "x" });
    assert_eq!(
        tool(&client, "view_open", two).await.unwrap_err(),
        "invalid_argument"
    );

    // The app is told, and its answer is the command's.
    let opening = {
        let client = NodeClient::new(Some(&url)).unwrap();
        tokio::spawn(async move {
            let args = json!({ "page": "files", "path": "/tmp/x" });
            tool(&client, "view_open", args).await
        })
    };
    let told = ws.next().await.unwrap().unwrap().into_data();
    assert_eq!(told[0], 0xb8);
    let command: Value = serde_json::from_slice(&told[5..]).unwrap();
    assert_eq!(
        command,
        json!({ "do": "open", "page": "files", "path": "/tmp/x", "node": node.node.id })
    );
    let answer = br#"{"ok":true,"view":{"page":"files"}}"#;
    ws.send(Message::Binary(
        [&[0xb1][..], &told[1..5], answer].concat().into(),
    ))
    .await
    .unwrap();
    assert_eq!(opening.await.unwrap(), Ok(json!({ "page": "files" })));

    // An app that does not answer.
    let args = json!({ "page": "files" });
    assert_eq!(
        tool(&client, "view_open", args).await.unwrap_err(),
        "timeout"
    );

    // Gone, it is no app.
    drop(ws);
    let mut shown = json!(null);
    for _ in 0..100 {
        shown = tool(&client, "view_show", json!({})).await.unwrap();
        if shown == json!([]) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(shown, json!([]));
    node.stop().await.unwrap();
}
