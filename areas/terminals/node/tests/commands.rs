//! The session tools over MCP against a real tmux on a scratch socket, on a
//! node holding only terminals. Linux and macOS only; skips without tmux
//! unless `PARVI_REQUIRE_TMUX=1`.
#![cfg(unix)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use parvi_testkit::node::Node as TestNode;
use parvi_testkit::tmux::{Server, capture, eventually, session_text};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A node with only terminals over a scratch tmux holding `alpha` at
/// 100x30, with `folder` as the launcher's one folder and `other` beside it.
struct Node {
    node: TestNode,
    folder: PathBuf,
    other: PathBuf,
    tmux: Server,
}

impl Node {
    async fn start(name: &str) -> Option<Node> {
        let tmux = Server::start_in(name)?;
        tmux.new_session("alpha", 100, 30);
        let (folder, other) = (tmux.dir().join("folder"), tmux.dir().join("other"));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let (folder, other) = (
            folder.canonicalize().unwrap(),
            other.canonicalize().unwrap(),
        );
        let launch = tmux.dir().join("launch.conf");
        std::fs::write(&launch, format!("folder {}\n", folder.display())).unwrap();
        let config = format!(
            "[terminals]\nsocket = {:?}\nlaunch = {:?}\n",
            tmux.socket_path(),
            launch
        );
        let node = TestNode::start(&[parvi_terminals::LISTING], &config)
            .await
            .unwrap();
        let sessions = node.sessions();
        eventually("alpha's pane read", || {
            let text = session_text(sessions.as_ref(), "alpha")?;
            text.contains('$').then_some(())
        })
        .await;
        Some(Node {
            node,
            folder,
            other,
            tmux,
        })
    }

    fn addr(&self) -> SocketAddr {
        self.node.address()
    }

    /// One tool call's answer: the tool's JSON, or its error's message.
    async fn call(&self, tool: &str, arguments: Value) -> Result<Value, String> {
        self.node
            .call(tool, arguments)
            .await
            .map_err(|error| error.message)
    }

    async fn stop(self) {
        self.node.stop().await;
    }
}

fn session_named<'a>(sessions: &'a Value, name: &str) -> &'a Value {
    sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("{name} in {sessions}"))
}

async fn post(addr: SocketAddr, body: &str, headers: &str) -> (u16, Value) {
    let (status, body) = http(addr, "POST", body, headers).await;
    let answer = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&body).unwrap_or(Value::String(body))
    };
    (status, answer)
}

async fn http(addr: SocketAddr, method: &str, body: &str, headers: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "{method} /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, body.to_owned())
}

/// The lifecycle, with the tools a node holding only terminals offers.
#[tokio::test(flavor = "multi_thread")]
async fn speaks_the_mcp_lifecycle_and_offers_every_session_tool() {
    let Some(node) = Node::start("lifecycle").await else {
        return;
    };
    let initialize = json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1" } },
    });
    let (status, answer) = post(node.addr(), &initialize.to_string(), "").await;
    assert_eq!(status, 200);
    assert_eq!(answer["id"], 0);
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answer["result"]["serverInfo"]["name"], "parvi");
    assert!(answer["result"]["capabilities"]["tools"].is_object());

    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let (status, answer) = post(node.addr(), &initialized.to_string(), "").await;
    assert_eq!(
        (status, answer),
        (202, Value::Null),
        "a notification gets no answer"
    );

    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
    let (_, answer) = post(node.addr(), &list.to_string(), "").await;
    let names: Vec<&str> = answer["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
            tool["name"].as_str().unwrap()
        })
        .filter(|name| !parvi_node_kit::own::COMMANDS.contains(name))
        .collect();
    assert_eq!(
        names,
        [
            "session_list",
            "session_read",
            "session_send",
            "session_launch",
            "session_copy",
            "session_history",
            "session_search",
            "session_keys",
            "session_rename",
            "session_kill",
            "session_mouse",
            "session_seen",
        ]
    );
    // Every tool reaches the fleet: `node` names where it runs.
    for tool in answer["result"]["tools"].as_array().unwrap() {
        assert_eq!(
            tool["inputSchema"]["properties"]["node"]["type"], "string",
            "{tool}"
        );
    }

    let ping = json!({ "jsonrpc": "2.0", "id": "p", "method": "ping" });
    let (_, answer) = post(node.addr(), &ping.to_string(), "").await;
    assert_eq!(answer, json!({ "jsonrpc": "2.0", "id": "p", "result": {} }));

    let unknown = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                          "params": { "name": "rm_rf", "arguments": {} } });
    let (_, answer) = post(node.addr(), &unknown.to_string(), "").await;
    assert_eq!(answer["error"]["code"], -32602);

    let (_, answer) = post(node.addr(), "{nope", "").await;
    assert_eq!(answer["error"]["code"], -32700);

    let (status, _) = http(node.addr(), "GET", "", "").await;
    assert_eq!(status, 405, "the server opens no streams");

    let (status, _) = post(
        node.addr(),
        &ping.to_string(),
        "Tailscale-Funnel-Request: ?1\r\n",
    )
    .await;
    assert_eq!(status, 403, "the public door is other until it has OAuth");
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_sessions_and_reads_a_screen() {
    let Some(node) = Node::start("read").await else {
        return;
    };
    node.tmux
        .run(&["send-keys", "-t", "alpha", "echo screen-$((6*7))", "Enter"]);

    let sessions = node.call("session_list", json!({})).await.unwrap();
    assert_eq!(sessions["node"], node.node.id());
    let alpha = session_named(&sessions, "alpha");
    assert_eq!(alpha["panes"][0]["cols"], 100);
    assert_eq!(alpha["panes"][0]["rows"], 30);

    let mut screen = Value::Null;
    for _ in 0..100 {
        screen = node
            .call("session_read", json!({ "session": "alpha" }))
            .await
            .unwrap();
        if screen["text"].as_str().unwrap().contains("screen-42") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        screen["text"].as_str().unwrap().contains("screen-42"),
        "{screen}"
    );
    assert_eq!(screen["session"], alpha["id"]);
    assert_eq!(screen["pane"], alpha["panes"][0]["id"]);
    let by_id = node
        .call(
            "session_read",
            json!({ "session": alpha["id"], "pane": alpha["panes"][0]["id"] }),
        )
        .await
        .unwrap();
    assert_eq!(by_id["pane"], screen["pane"]);

    let missing = node
        .call("session_read", json!({ "session": "nobody" }))
        .await;
    assert_eq!(missing.unwrap_err(), "no session nobody");
    let wrong_pane = node
        .call("session_read", json!({ "session": "alpha", "pane": 99999 }))
        .await;
    assert!(wrong_pane.unwrap_err().contains("no such pane"));
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sends_text_into_a_session() {
    let Some(node) = Node::start("send").await else {
        return;
    };
    let sent = node
        .call(
            "session_send",
            json!({ "session": "alpha", "text": "echo sent-$((6*7))" }),
        )
        .await
        .unwrap();
    assert_eq!(sent["sent"], true);
    eventually("the echo ran", || {
        capture(&node.tmux, "alpha")
            .lines()
            .any(|line| line.trim() == "sent-42")
            .then_some(())
    })
    .await;

    node.call(
        "session_send",
        json!({ "session": "alpha", "text": "unsent-line", "enter": false }),
    )
    .await
    .unwrap();
    eventually("the text typed", || {
        capture(&node.tmux, "alpha")
            .contains("unsent-line")
            .then_some(())
    })
    .await;
    assert!(!capture(&node.tmux, "alpha").contains("unsent-line: not found"));
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn presses_keys_reads_history_searches_and_kills() {
    let Some(node) = Node::start("keys").await else {
        return;
    };
    node.call(
        "session_keys",
        json!({ "session": "alpha", "keys": ["echo ", "keyed-$((6*7))", "Enter"] }),
    )
    .await
    .unwrap();
    eventually("the keyed echo ran", || {
        capture(&node.tmux, "alpha")
            .lines()
            .any(|line| line.trim() == "keyed-42")
            .then_some(())
    })
    .await;

    let history = node
        .call("session_history", json!({ "session": "alpha" }))
        .await
        .unwrap();
    let text = history["text"].as_str().unwrap();
    assert!(text.lines().any(|line| line.trim() == "keyed-42"), "{text}");
    assert!(!text.contains('\x1b'), "plain text: {text:?}");
    let last = node
        .call("session_history", json!({ "session": "alpha", "last": 1 }))
        .await
        .unwrap();
    assert_eq!(last["text"].as_str().unwrap().lines().count(), 1, "{last}");

    let found = node
        .call("session_search", json!({ "query": "keyed-42" }))
        .await
        .unwrap();
    let matches = found["matches"].as_array().unwrap();
    assert!(
        matches
            .iter()
            .any(|m| m["text"].as_str().unwrap().trim() == "keyed-42"),
        "{found}"
    );
    let alpha = matches[0]["session"].as_str().unwrap().to_owned();

    let seen = node
        .call("session_seen", json!({ "session": "alpha" }))
        .await
        .unwrap();
    assert_eq!(seen["report"], false, "alpha has made no report");

    let killed = node
        .call("session_kill", json!({ "session": "alpha" }))
        .await
        .unwrap();
    assert_eq!(killed["session"], alpha);
    eventually("alpha is gone", || {
        let listed = node
            .tmux
            .command()
            .args(["has-session", "-t", "alpha"])
            .status()
            .ok()?;
        (!listed.success()).then_some(())
    })
    .await;
    node.stop().await;
}

/// #184: a rename acts on exactly the session it names, answers once the
/// list has the new name, and refuses a name taken on its server or a
/// session the node does not hold.
#[tokio::test(flavor = "multi_thread")]
async fn renames_a_session_unless_the_name_is_taken() {
    let Some(node) = Node::start("rename").await else {
        return;
    };
    let sessions = node.node.sessions();
    node.tmux.new_session("taken", 80, 24);
    eventually("taken is listed", || {
        let listed = sessions.list();
        listed.iter().any(|s| s.name == "taken").then_some(())
    })
    .await;
    let alpha = node
        .call("session_read", json!({ "session": "alpha" }))
        .await
        .unwrap()["session"]
        .clone();
    let mut renames = sessions.renames();
    assert_eq!(renames.len(), 1, "one server: the node's own");
    let (whose, mut renames) = renames.remove(0);
    assert_eq!(whose, None);

    let renamed = node
        .call(
            "session_rename",
            json!({ "session": "alpha", "name": "omega" }),
        )
        .await
        .unwrap();
    assert_eq!(
        (&renamed["session"], &renamed["name"]),
        (&alpha, &json!("omega"))
    );
    let read = node
        .call("session_read", json!({ "session": "omega" }))
        .await
        .unwrap();
    assert_eq!(read["session"], alpha, "listed under the new name at once");
    let listed = node.call("session_list", json!({})).await.unwrap();
    let mut listed: Vec<&str> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    listed.sort();
    assert_eq!(listed, ["omega", "taken"], "no other session renamed");
    assert_eq!(
        renames.try_recv().unwrap(),
        ("alpha".to_string(), "omega".to_string())
    );
    assert!(
        renames.try_recv().is_err(),
        "told once, though every control client heard it (#237)"
    );

    let names = || node.tmux.run(&["list-sessions", "-F", "#{session_name}"]);
    let taken = node
        .call(
            "session_rename",
            json!({ "session": "omega", "name": "taken" }),
        )
        .await
        .unwrap_err();
    assert!(taken.contains("duplicate session"), "{taken}");
    let unknown = node
        .call(
            "session_rename",
            json!({ "session": "nobody", "name": "free" }),
        )
        .await
        .unwrap_err();
    assert!(unknown.contains("no session nobody"), "{unknown}");
    let bad = node
        .call(
            "session_rename",
            json!({ "session": "omega", "name": "a:b" }),
        )
        .await
        .unwrap_err();
    assert!(bad.contains("letters, digits"), "{bad}");
    let mut left: Vec<String> = names().lines().map(str::to_owned).collect();
    left.sort();
    assert_eq!(left, ["omega", "taken"]);
    node.stop().await;
}

/// #184: the mouse toggle sets tmux's mouse option for the whole server,
/// as CHROTE's does, and takes away tmux's right-click menus so a right
/// click reaches the app's own menu.
#[tokio::test(flavor = "multi_thread")]
async fn turns_the_mouse_on_and_off_for_the_whole_server() {
    let Some(node) = Node::start("mouse").await else {
        return;
    };
    let mouse = || node.tmux.run(&["show-options", "-gv", "mouse"]);
    let menu = || node.tmux.run(&["list-keys", "-T", "root"]);
    node.tmux
        .run(&["bind-key", "-n", "MouseDown3Pane", "display-menu", "x"]);
    assert!(menu().contains("MouseDown3Pane"));

    let on = node
        .call("session_mouse", json!({ "mouse": "on" }))
        .await
        .unwrap();
    assert_eq!(
        (&on["mouse"], &on["applied"], &on["total"]),
        (&json!("on"), &json!(1), &json!(1))
    );
    assert_eq!(mouse().trim(), "on");
    assert!(!menu().contains("MouseDown3Pane"), "{}", menu());

    node.call("session_mouse", json!({ "mouse": "off" }))
        .await
        .unwrap();
    assert_eq!(mouse().trim(), "off");
    let bad = node
        .call("session_mouse", json!({ "mouse": "maybe" }))
        .await
        .unwrap_err();
    assert!(bad.contains("on or off"), "{bad}");
    assert_eq!(mouse().trim(), "off");
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn launches_an_agent_in_any_folder() {
    let Some(node) = Node::start("launch").await else {
        return;
    };
    let launched = node
        .call(
            "session_launch",
            json!({ "harness": "shell", "folder": node.folder, "name": "fresh" }),
        )
        .await
        .unwrap();
    let id = launched["session"].as_str().unwrap();
    let sessions = node.call("session_list", json!({})).await.unwrap();
    assert_eq!(
        session_named(&sessions, "fresh")["id"],
        id,
        "listed on answer"
    );
    assert_eq!(
        node.tmux.display("fresh", "#{pane_current_path}"),
        node.folder.display().to_string()
    );

    assert_eq!(node.tmux.window_size("fresh"), "200x50");
    node.call("session_launch", json!({ "harness": "shell", "folder": node.folder, "name": "sized", "cols": 91, "rows": 31 })).await.unwrap();
    assert_eq!(node.tmux.window_size("sized"), "91x31");
    assert!(
        node.call(
            "session_launch",
            json!({ "harness": "shell", "folder": node.folder, "name": "incomplete", "cols": 91 })
        )
        .await
        .is_err()
    );

    for (name, dimensions, expected) in [
        ("cli-default", vec![], "200x50"),
        ("cli-sized", vec!["--cols", "86", "--rows", "29"], "86x29"),
    ] {
        let folder = node.folder.to_str().unwrap();
        let url = node.node.url();
        let mut args = vec!["launch", "shell", folder, name, "--node", &url];
        args.extend(dimensions);
        let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
        assert_eq!(
            parvi_node_kit::cli::run("session", &args, &mut Vec::new())
                .await
                .unwrap(),
            0
        );
        assert_eq!(node.tmux.window_size(name), expected);
    }

    // Copy uses the source's current folder and chooses its next free family name.
    // A client connecting has every session's facts read again.
    let (_client, _) = tokio_tungstenite::connect_async(node.node.ws())
        .await
        .unwrap();
    let sessions = node.node.sessions();
    eventually("fresh session facts", || {
        sessions
            .list()
            .iter()
            .find(|s| s.name == "fresh")
            .and_then(|s| s.facts.as_ref())
            .filter(|f| !f.path.is_empty())
            .map(|_| ())
    })
    .await;
    let copied = node
        .call("session_copy", json!({"session":"fresh"}))
        .await
        .unwrap();
    assert!(copied["session"].is_string());
    assert_eq!(
        node.tmux.display("fresh-2", "#{pane_current_path}"),
        node.folder.display().to_string()
    );

    // No permissions in Parvi: any folder the node's user reaches will do.
    node.call(
        "session_launch",
        json!({ "harness": "shell", "folder": node.other, "name": "outside" }),
    )
    .await
    .unwrap();
    assert_eq!(
        node.tmux.display("outside", "#{pane_current_path}"),
        node.other.display().to_string()
    );
    let unknown = node
        .call(
            "session_launch",
            json!({ "harness": "nope", "folder": node.folder, "name": "bad" }),
        )
        .await;
    assert!(unknown.unwrap_err().contains("no harness"));
    let names = node.tmux.run(&["list-sessions", "-F", "#{session_name}"]);
    assert!(!names.contains("bad"), "{names}");
    node.stop().await;
}

/// A seat, as a mission runs its agents: created on the node's own server
/// with its environment, found by name, followed, typed into and read, its
/// program's pid known, and killed only as the session that was created.
#[tokio::test(flavor = "multi_thread")]
async fn seats_run_through_the_sessions_terminals_provide() {
    let Some(node) = Node::start("seats").await else {
        return;
    };
    let sessions = node.node.sessions();
    let spec = parvi_term::SessionSpec {
        name: "seat".into(),
        cwd: node.folder.clone(),
        env: [("SEAT_WORD".to_string(), "kept".to_string())].into(),
        command: vec!["sh".into()],
        cols: 90,
        rows: 20,
    };
    let seat = sessions.create(spec).await.unwrap();
    assert_eq!(sessions.find("seat").await.unwrap(), Some(seat.clone()));
    assert_eq!(
        sessions.find("sea").await.unwrap(),
        None,
        "names match exactly"
    );
    sessions.verify(&seat).await.unwrap();
    let pane = seat.pane_id;
    let mut followed = eventually("the seat followed", || sessions.follow(pane)).await;
    assert!(sessions.pid(pane).await.unwrap() > 1);
    sessions
        .input(pane, b"echo seat-$SEAT_WORD\r".to_vec())
        .await
        .unwrap();
    let update = tokio::time::timeout(Duration::from_secs(5), followed.updates.recv()).await;
    assert!(
        matches!(update, Ok(Ok(_))),
        "the seat's screen changes reach it"
    );
    eventually("the seat's echo on its screen", || {
        let screen = sessions.screen(pane)?;
        (screen.rows.iter())
            .any(|row| {
                let line: String = row.iter().map(|cell| cell.text.as_str()).collect();
                line.trim() == "seat-kept"
            })
            .then_some(())
    })
    .await;
    sessions.kill(&seat).await.unwrap();
    assert!(sessions.verify(&seat).await.is_err());
    assert_eq!(sessions.find("seat").await.unwrap(), None);
    node.stop().await;
}

/// The command-line client reaches the same session operations on a test node.
#[tokio::test(flavor = "multi_thread")]
async fn cli_manages_and_reads_a_session_on_the_test_node() {
    let Some(node) = Node::start("cli-journey").await else {
        return;
    };
    let url = node.node.url();
    let run = |arguments: Vec<String>| {
        let url = url.clone();
        async move {
            let mut args = arguments;
            args.extend(["--node".into(), url, "--json".into()]);
            let mut output = Vec::new();
            assert_eq!(
                parvi_node_kit::cli::run("session", &args, &mut output)
                    .await
                    .unwrap(),
                0
            );
            serde_json::from_slice::<Value>(&output).unwrap()
        }
    };
    run(vec![
        "launch".into(),
        "shell".into(),
        node.folder.to_string_lossy().into_owned(),
        "cli-journey".into(),
    ])
    .await;
    assert_eq!(
        node.tmux.display("cli-journey", "#{pane_current_path}"),
        node.folder.display().to_string()
    );
    run(vec![
        "send".into(),
        "cli-journey".into(),
        "--text".into(),
        "echo journey-$((6*7))".into(),
    ])
    .await;
    eventually("CLI output", || {
        capture(&node.tmux, "cli-journey")
            .contains("journey-42")
            .then_some(())
    })
    .await;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let read = run(vec!["read".into(), "cli-journey".into()]).await;
        if read["text"].as_str().unwrap().contains("journey-42") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "CLI screen never caught up"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let history = run(vec!["history".into(), "cli-journey".into()]).await;
    assert!(history["text"].as_str().unwrap().contains("journey-42"));
    let listed = run(vec!["list".into()]).await;
    assert_eq!(session_named(&listed, "cli-journey")["name"], "cli-journey");
    run(vec![
        "rename".into(),
        "cli-journey".into(),
        "renamed-cli".into(),
    ])
    .await;
    run(vec!["seen".into(), "renamed-cli".into()]).await;
    run(vec!["kill".into(), "renamed-cli".into()]).await;
    eventually("CLI killed session", || {
        (!node
            .tmux
            .run(&["list-sessions", "-F", "#{session_name}"])
            .lines()
            .any(|s| s == "renamed-cli"))
        .then_some(())
    })
    .await;
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_launches_on_the_offered_users_scratch_server() {
    let (Some(own), Some(other)) = (
        Server::start_in("cli-own-user"),
        Server::start_in("cli-other-user"),
    ) else {
        return;
    };
    own.new_session("keep", 80, 24);
    other.new_session("other-keep", 80, 24);
    let user = parvi_testkit::tmux::whoami();
    let launch = own.dir().join("launch.conf");
    std::fs::write(&launch, "folder /tmp").unwrap();
    let config = format!(
        "[terminals]\nsocket = {:?}\nlaunch = {:?}\nusers = {{ {user} = {:?} }}\n",
        own.socket_path(),
        launch,
        other.socket_path()
    );
    let node = TestNode::start(&[parvi_terminals::LISTING], &config)
        .await
        .unwrap();
    let url = node.url();
    let args = [
        "launch",
        "shell",
        "/tmp",
        "other-cli",
        "--user",
        &user,
        "--node",
        &url,
    ]
    .map(str::to_owned);
    assert_eq!(
        parvi_node_kit::cli::run("session", &args, &mut Vec::new())
            .await
            .unwrap(),
        0
    );
    assert!(
        other
            .run(&["list-sessions", "-F", "#{session_name}"])
            .lines()
            .any(|s| s == "other-cli")
    );
    assert!(
        !own.run(&["list-sessions", "-F", "#{session_name}"])
            .lines()
            .any(|s| s == "other-cli")
    );
    node.stop().await;
}
