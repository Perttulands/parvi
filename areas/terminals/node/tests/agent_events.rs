//! A harness's own hook reports to the node, over the socket, against a
//! real tmux. Alone in its binary: it points `CODEX_HOME` away from the
//! operator's. Linux and macOS only; skips without tmux unless
//! `PARVI_REQUIRE_TMUX=1`.
#![cfg(unix)]

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use parvi_terminals_contract::{self as protocol, Up};
use parvi_testkit::node::Node;
use parvi_testkit::tmux::Server as Tmux;
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The next binary frame, or a panic after five seconds.
async fn next_frame(ws: &mut Ws) -> Vec<u8> {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a frame")
            .expect("socket closed")
            .unwrap();
        if let Message::Binary(bytes) = message {
            return bytes.to_vec();
        }
    }
}

/// Reads frames until one satisfies `check`.
async fn until(ws: &mut Ws, what: &str, mut check: impl FnMut(&[u8]) -> bool) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        let frame = next_frame(ws).await;
        if check(&frame) {
            return frame;
        }
    }
}

/// Agent events over the socket (#205): a launch asked to notify types the
/// harness's hook, the report the hook makes reaches every client as the
/// session's event, and a client's look makes it seen.
#[tokio::test(flavor = "multi_thread")]
async fn an_agents_report_reaches_the_client_until_it_is_seen() {
    let Some(tmux) = Tmux::start() else {
        return;
    };
    tmux.run(&["new-session", "-d", "-s", "keep"]);
    let dir = parvi_testkit::fresh_dir("parvi-hooks");
    // No Codex config of the operator's: its own notify would be passed on.
    // SAFETY: this test is alone in its binary, and sets it before any
    // thread that could read the environment starts.
    unsafe { std::env::set_var("CODEX_HOME", dir.join("no-codex")) };
    // A fake by its path: the node hooks it as Codex by its name, and the
    // real one is never typed (#215).
    let codex = dir.join("codex");
    parvi_testkit::tmux::write_script(&codex, "#!/bin/sh\nexit 0\n");
    let launch = dir.join("launch.conf");
    std::fs::write(
        &launch,
        format!("harness codex {}\nfolder /tmp", codex.display()),
    )
    .unwrap();
    let config = format!(
        "[terminals]\nsocket = {:?}\nlaunch = {:?}\n",
        tmux.socket_path(),
        launch
    );
    let node = Node::start(&[parvi_terminals::LISTING], &config)
        .await
        .unwrap();
    let addr = node.address();
    // The hook runs this binary: the node's own.
    let exe = std::env::current_exe().unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();

    let launch = protocol::Launch {
        harness: "codex".into(),
        folder: "/tmp".into(),
        name: "told".into(),
        flags: "--search".into(),
        user: String::new(),
        notify: true,
        size: None,
    };
    ws.send(Message::Binary(
        protocol::encode_up(&Up::Launch { id: 1, launch }).into(),
    ))
    .await
    .unwrap();
    let done = until(&mut ws, "the launch answered", |f| f[0] == protocol::DONE).await;
    assert_eq!(&done[1..6], &[1, 0, 0, 0, 1]);
    let session = String::from_utf8(done[6..].to_vec()).unwrap();
    let url = format!(
        "http://{addr}/api/agent/event?session=%24{}&event=finished",
        &session[1..]
    );
    let typed = format!(
        "{} -c 'notify=[\"{}\",\"hook\",\"{url}\"]' --search",
        codex.display(),
        exe.display()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !tmux
        .run(&["capture-pane", "-p", "-J", "-t", "told"])
        .contains(&typed)
    {
        assert!(Instant::now() < deadline, "the hook was not typed: {typed}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // What the hook runs, with Codex's payload as the last argument.
    let args = [
        url,
        r#"{"type":"agent-turn-complete","last-assistant-message":"All done."}"#.to_owned(),
    ];
    tokio::task::spawn_blocking(move || parvi_node_kit::hook::report(&args))
        .await
        .unwrap();
    let event = r#""event":{"kind":"finished","at":"#;
    let reported = until(&mut ws, "the report", |f| {
        let text = String::from_utf8_lossy(&f[1..]);
        f[0] == protocol::SESSIONS
            && text.contains(event)
            && text.contains(r#""seen":false,"summary":"All done."}"#)
    })
    .await;
    let text = String::from_utf8_lossy(&reported[1..]).into_owned();
    let at = &text[text.find(event).unwrap() + event.len()..];
    let at: u64 = at[..at.find(',').unwrap()].parse().unwrap();

    ws.send(Message::Binary(
        protocol::encode_up(&Up::Seen { session, event: at }).into(),
    ))
    .await
    .unwrap();
    until(&mut ws, "the look", |f| {
        f[0] == protocol::SESSIONS && String::from_utf8_lossy(&f[1..]).contains(r#""seen":true"#)
    })
    .await;

    drop(ws);
    node.stop().await;
    std::fs::remove_dir_all(dir).unwrap();
}
