//! The terminals area on the socket against a real tmux: a headless client
//! connects to a node holding only terminals, watches a pane and types into
//! it. Linux and macOS only; skips without tmux unless `PARVI_REQUIRE_TMUX=1`.
#![cfg(unix)]

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use parvi_core::Hello;
use parvi_terminals_contract::{self as protocol, Up};
use parvi_testkit::node::Node;
use parvi_testkit::tmux::Server as Tmux;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

/// A node with only terminals, following `tmux`'s server, with `launch` as
/// its launcher's file.
async fn node(tmux: &Tmux, launch: &str) -> Node {
    let file = tmux.dir().join("launch.conf");
    std::fs::write(&file, launch).unwrap();
    let config = format!(
        "[terminals]\nsocket = {:?}\nlaunch = {:?}\n",
        tmux.socket_path(),
        file
    );
    Node::start(&[parvi_terminals::LISTING], &config)
        .await
        .unwrap()
}

/// The sessions `session_list` lists.
async fn listed(node: &Node) -> Vec<Value> {
    let list = node.call("session_list", json!({})).await.unwrap();
    list["sessions"].as_array().unwrap().clone()
}

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

fn pane_of(frame: &[u8]) -> u32 {
    u32::from_le_bytes(frame[1..5].try_into().unwrap())
}

/// Every connection hears the hello before anything else: which node, device
/// and environment answered, and which protocol it speaks. Needs no tmux
/// server: a node with no sessions still says hello.
#[tokio::test(flavor = "multi_thread")]
async fn every_connection_starts_with_the_hello() {
    let dir = parvi_testkit::fresh_dir("parvi-hello");
    let config = format!(
        "[node]\ndevice = \"test-rig\"\n[terminals]\nsocket = {:?}\n",
        dir.join("tmux")
    );
    let node = Node::start(&[parvi_terminals::LISTING], &config)
        .await
        .unwrap();
    for _ in 0..2 {
        let (mut ws, _) = tokio_tungstenite::connect_async(node.ws()).await.unwrap();
        let hello = next_frame(&mut ws).await;
        assert_eq!(hello[0], parvi_core::frame::HELLO, "the hello comes first");
        let hello = Hello::decode(&hello).unwrap();
        assert_eq!(hello.node, node.id());
        assert_eq!(hello.device, "test-rig");
        assert_eq!(hello.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(hello.protocol, parvi_core::PROTOCOL);
        assert_eq!(next_frame(&mut ws).await[0], protocol::SESSIONS);
    }
    node.stop().await;
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_watches_and_types_into_a_pane() {
    let Some(tmux) = Tmux::start() else { return };
    tmux.run(&[
        "new-session",
        "-d",
        "-s",
        "alpha",
        "-x",
        "100",
        "-y",
        "30",
        "env PS1='$ ' sh",
    ]);
    tmux.run(&["send-keys", "-t", "alpha", "echo seeded-marker", "Enter"]);
    let pane: u32 = tmux
        .run(&["display-message", "-p", "-t", "alpha", "#{pane_id}"])
        .trim()
        .trim_start_matches('%')
        .parse()
        .unwrap();

    let node = node(&tmux, "").await;
    let addr = node.address();

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();

    // The session list arrives on connect and again as the engine finds panes.
    until(&mut ws, "alpha in the session list", |f| {
        f[0] == protocol::SESSIONS
            && std::str::from_utf8(&f[1..]).unwrap().contains(&format!(
                "\"name\":\"alpha\",\"panes\":[{{\"id\":{pane},\"cols\":100,\"rows\":30}}]"
            ))
    })
    .await;

    // Subscribing gives the whole screen first, at the pane's own size.
    ws.send(Message::Binary(
        protocol::encode_up(&Up::Subscribe(pane)).into(),
    ))
    .await
    .unwrap();
    let screen = until(&mut ws, "the screen", |f| f[0] == protocol::SCREEN).await;
    assert_eq!(pane_of(&screen), pane);
    assert_eq!(u16::from_le_bytes([screen[5], screen[6]]), 100);
    assert_eq!(u16::from_le_bytes([screen[7], screen[8]]), 30);
    let mut seen = vt100::Parser::new(30, 100, 0);
    seen.process(&screen[9..]);
    assert!(seen.screen().contents().contains("seeded-marker"));

    // Typing arrives as output frames; time each keystroke's round trip.
    let mut round_trips = Vec::new();
    for (i, key) in "echo hi-$((6*7))".bytes().enumerate() {
        let sent = Instant::now();
        ws.send(Message::Binary(
            protocol::encode_up(&Up::Input(pane, vec![key])).into(),
        ))
        .await
        .unwrap();
        let echoed = until(&mut ws, &format!("the echo of key {i}"), |f| {
            f[0] == protocol::OUTPUT && pane_of(f) == pane
        })
        .await;
        round_trips.push(sent.elapsed());
        seen.process(&echoed[5..]);
    }
    ws.send(Message::Binary(
        protocol::encode_up(&Up::Input(pane, b"\r".to_vec())).into(),
    ))
    .await
    .unwrap();
    while !seen.screen().contents().contains("hi-42") {
        let frame = until(&mut ws, "hi-42", |f| f[0] == protocol::OUTPUT).await;
        seen.process(&frame[5..]);
    }
    round_trips.sort();
    eprintln!(
        "FINDING keystroke round trip, client socket to echo (n={}): p50 {:?}, max {:?}",
        round_trips.len(),
        round_trips[round_trips.len() / 2],
        round_trips.last().unwrap()
    );

    // A killed pane is reported gone.
    tmux.run(&["new-session", "-d", "-s", "keep"]);
    tmux.run(&["kill-session", "-t", "alpha"]);
    until(&mut ws, "pane gone", |f| {
        f[0] == protocol::PANE_GONE && pane_of(f) == pane
    })
    .await;

    // And the client never changed the window's size.
    assert!(
        tmux.run(&["list-sessions", "-F", "#{session_name}"])
            .contains("keep")
    );
    drop(ws);
    node.stop().await;
}

/// Issue #15 over the socket: a client claims a pane's seat at its grid, the
/// session list tells it and every other client who holds the seat, and the
/// seat is given back when the client goes.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_claims_the_seat_and_gives_it_back_when_it_goes() {
    let Some(tmux) = Tmux::start() else {
        return;
    };
    tmux.run(&["new-session", "-d", "-s", "alpha", "-x", "100", "-y", "30"]);
    let pane: u32 = tmux
        .run(&["display-message", "-p", "-t", "alpha", "#{pane_id}"])
        .trim()
        .trim_start_matches('%')
        .parse()
        .unwrap();
    let node = node(&tmux, "").await;
    let addr = node.address();
    let connect = move || async move {
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
            .await
            .unwrap();
        until(&mut ws, "the pane listed", |f| {
            f[0] == protocol::SESSIONS && std::str::from_utf8(&f[1..]).unwrap().contains("\"cols\"")
        })
        .await;
        ws
    };
    let seat = |frame: &[u8], seat: &str| {
        frame[0] == protocol::SESSIONS && std::str::from_utf8(&frame[1..]).unwrap().contains(seat)
    };
    let mut claimer = connect().await;
    let mut other = connect().await;

    claimer
        .send(Message::Binary(
            protocol::encode_up(&Up::Claim(pane, 90, 25)).into(),
        ))
        .await
        .unwrap();
    until(&mut claimer, "seat mine", |f| seat(f, "\"seat\":\"mine\"")).await;
    until(&mut other, "seat other", |f| seat(f, "\"seat\":\"other\"")).await;
    let size = || {
        tmux.run(&[
            "display-message",
            "-p",
            "-t",
            "alpha",
            "#{window_width}x#{window_height}",
        ])
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while size().trim() != "90x25" {
        assert!(Instant::now() < deadline, "the window never took the claim");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    drop(claimer);
    until(&mut other, "the seat free", |f| {
        f[0] == protocol::SESSIONS && !std::str::from_utf8(&f[1..]).unwrap().contains("seat")
    })
    .await;
    let ours = tmux.run(&["list-clients", "-F", "#{client_flags}"]);
    assert!(
        ours.contains("ignore-size"),
        "our client stopped sizing: {ours}"
    );

    tmux.run(&["set-option", "-w", "-t", "alpha", "window-size", "manual"]);
    let before = size();
    other
        .send(Message::Binary(
            protocol::encode_up(&Up::Claim(pane, 40, 10)).into(),
        ))
        .await
        .unwrap();
    let refused = until(&mut other, "pinned claim refused", |f| {
        f[0] == protocol::CLAIM_REFUSED
    })
    .await;
    assert_eq!(pane_of(&refused), pane);
    assert!(
        std::str::from_utf8(&refused[5..])
            .unwrap()
            .contains("pinned")
    );
    assert_eq!(size(), before);

    drop(other);
    node.stop().await;
}

/// Issue #17 over the socket: the overview sends thumbnails while it is on,
/// at most twice a second per pane, and nothing once it is off.
#[tokio::test(flavor = "multi_thread")]
async fn the_overview_sends_thumbnails_until_it_stops() {
    let Some(tmux) = Tmux::start() else {
        return;
    };
    tmux.run(&[
        "new-session",
        "-d",
        "-s",
        "busy",
        "-x",
        "60",
        "-y",
        "10",
        "while :; do echo busy-$RANDOM; sleep 0.05; done",
    ]);
    tmux.run(&[
        "new-session",
        "-d",
        "-s",
        "quiet",
        "-x",
        "50",
        "-y",
        "8",
        "env PS1='$ ' sh",
    ]);
    tmux.run(&["send-keys", "-t", "quiet", "echo quiet-marker", "Enter"]);
    let node = node(&tmux, "").await;
    let addr = node.address();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let on = |on| Message::Binary(protocol::encode_up(&Up::Overview(on)).into());

    ws.send(on(true)).await.unwrap();
    let quiet = until(&mut ws, "the quiet session's thumbnail", |f| {
        f[0] == protocol::THUMBNAIL
            && std::str::from_utf8(&f[9..])
                .unwrap()
                .contains("quiet-marker")
    })
    .await;
    assert_eq!(u16::from_le_bytes([quiet[5], quiet[6]]), 50);
    assert_eq!(u16::from_le_bytes([quiet[7], quiet[8]]), 8);

    // Over two seconds the busy pane, writing twenty times a second, is sent
    // at most five times (two a second, plus one for the tick in flight); the
    // quiet one, which has not changed, not again.
    let quiet_pane = pane_of(&quiet);
    let mut busy = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while let Ok(Some(Ok(message))) = tokio::time::timeout_at(deadline, ws.next()).await {
        if let Message::Binary(f) = message
            && f[0] == protocol::THUMBNAIL
        {
            assert_ne!(pane_of(&f), quiet_pane, "an unchanged pane was sent again");
            busy += 1;
        }
    }
    assert!((2..=5).contains(&busy), "busy thumbnails in 2 s: {busy}");

    // Off means off, once what was already on its way has arrived.
    ws.send(on(false)).await.unwrap();
    let settle = tokio::time::Instant::now() + Duration::from_millis(200);
    while let Ok(Some(_)) = tokio::time::timeout_at(settle, ws.next()).await {}
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
    while let Ok(Some(Ok(message))) = tokio::time::timeout_at(deadline, ws.next()).await {
        if let Message::Binary(f) = message {
            assert_ne!(
                f[0],
                protocol::THUMBNAIL,
                "a thumbnail after the overview stopped"
            );
        }
    }

    drop(ws);
    node.stop().await;
}
/// Issue #20 over the socket: the client gets the launcher on connect, a
/// launch answers with the new session's id once it is listed, and a delete
/// answers once the session is gone.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_launches_and_deletes_a_session() {
    let Some(tmux) = Tmux::start() else {
        return;
    };
    tmux.run(&["new-session", "-d", "-s", "keep"]);
    let node = node(&tmux, "folder /tmp").await;
    let addr = node.address();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let config = until(&mut ws, "the launcher", |f| f[0] == protocol::LAUNCHER).await;
    assert!(
        std::str::from_utf8(&config[1..])
            .unwrap()
            .contains("\"folders\":[\"/tmp\"]")
    );

    let launch = protocol::Launch {
        harness: "shell".into(),
        folder: "/tmp".into(),
        name: "fresh".into(),
        flags: String::new(),
        user: String::new(),
        notify: false,
        // The grid of the window it is launched into (#161).
        size: Some((90, 28)),
    };
    ws.send(Message::Binary(
        protocol::encode_up(&Up::Launch { id: 9, launch }).into(),
    ))
    .await
    .unwrap();
    let done = until(&mut ws, "the launch answered", |f| f[0] == protocol::DONE).await;
    assert_eq!(
        &done[1..6],
        &[9, 0, 0, 0, 1],
        "{:?}",
        String::from_utf8_lossy(&done[6..])
    );
    let session = String::from_utf8(done[6..].to_vec()).unwrap();
    assert!(
        listed(&node)
            .await
            .iter()
            .any(|s| s["id"] == session.as_str() && s["name"] == "fresh"),
        "listed before the answer"
    );
    assert_eq!(tmux.window_size("fresh"), "90x28");

    ws.send(Message::Binary(
        protocol::encode_up(&Up::Delete { id: 10, session }).into(),
    ))
    .await
    .unwrap();
    let done = until(&mut ws, "the delete answered", |f| f[0] == protocol::DONE).await;
    assert_eq!(&done[1..6], &[10, 0, 0, 0, 1]);
    assert_eq!(
        tmux.run(&["list-sessions", "-F", "#{session_name}"]).trim(),
        "keep"
    );

    drop(ws);
    node.stop().await;
}
/// Issue #22 over the socket: matches stream and then the end, and the
/// history behind a match comes back read-only.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_searches_scrollback_and_reads_history() {
    let Some(tmux) = Tmux::start() else {
        return;
    };
    tmux.new_session("alpha", 80, 10);
    // Wait for the shell before typing: otherwise its first prompt can be
    // drawn into the command's output, merging the marker with the command.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let screen = tmux.run(&["capture-pane", "-p", "-t", "alpha"]);
        if screen.lines().any(|line| line.trim_end().ends_with('$')) {
            break;
        }
        assert!(Instant::now() < deadline, "shell did not become ready");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tmux.run(&[
        "send-keys",
        "-t",
        "alpha",
        "echo found-me; seq 1 100",
        "Enter",
    ]);
    let node = node(&tmux, "").await;
    let addr = node.address();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    until(&mut ws, "alpha listed", |f| {
        f[0] == protocol::SESSIONS && std::str::from_utf8(&f[1..]).unwrap().contains("\"cols\"")
    })
    .await;
    // Search tmux's history only after seq has scrolled the marker off.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let screen = tmux.run(&["capture-pane", "-p", "-t", "alpha"]);
        if screen.lines().any(|line| line.trim() == "100") {
            break;
        }
        assert!(Instant::now() < deadline, "seq did not finish");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let search = |query: &str| {
        Message::Binary(
            protocol::encode_up(&Up::Search {
                id: 1,
                query: query.into(),
            })
            .into(),
        )
    };
    ws.send(search("found-me")).await.unwrap();
    let mut lines = Vec::new();
    let end = loop {
        let f = until(&mut ws, "search frames", |f| {
            f[0] == protocol::MATCH || f[0] == protocol::SEARCHED
        })
        .await;
        if f[0] == protocol::SEARCHED {
            break f;
        }
        assert_eq!(&f[1..5], &[1, 0, 0, 0]);
        let pane = u32::from_le_bytes(f[5..9].try_into().unwrap());
        let line = u32::from_le_bytes(f[9..13].try_into().unwrap());
        lines.push((pane, line, String::from_utf8(f[13..].to_vec()).unwrap()));
    };
    let matches = u32::from_le_bytes(end[5..9].try_into().unwrap());
    assert_eq!(matches as usize, lines.len());
    let (pane, line, _) = lines
        .iter()
        // The output line, not the typed one.
        .find(|(_, _, text)| {
            text.split('\n')
                .nth(1)
                .is_some_and(|l| l.ends_with("found-me") && !l.contains("echo"))
        })
        .expect("the marker's output line")
        .clone();

    ws.send(Message::Binary(
        protocol::encode_up(&Up::History { pane, id: 2 }).into(),
    ))
    .await
    .unwrap();
    let history = until(&mut ws, "the history", |f| f[0] == protocol::HISTORY).await;
    assert_eq!(&history[1..5], &[2, 0, 0, 0]);
    assert_eq!(history[9], 1);
    let text = String::from_utf8(history[10..].to_vec()).unwrap();
    assert!(
        text.lines()
            .nth(line as usize)
            .unwrap()
            .contains("found-me")
    );

    drop(ws);
    node.stop().await;
}
/// Issue #39 over the socket: two tmux servers, the service user's and
/// another user's, show as one list; a launch, a pane's input, a search and
/// a delete each reach the right one. Both scratch servers run as whoever
/// runs the test, so that user plays the other user too.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_sees_and_uses_another_users_server() {
    let (Some(own), Some(other)) = (Tmux::start(), Tmux::start()) else {
        return;
    };
    own.run(&["new-session", "-d", "-s", "mine", "env PS1='$ ' sh"]);
    other.run(&["new-session", "-d", "-s", "theirs", "env PS1='$ ' sh"]);
    let user = parvi_testkit::tmux::whoami();
    let launch = own.dir().join("launch.conf");
    std::fs::write(&launch, "folder /tmp").unwrap();
    let config = format!(
        "[terminals]\nsocket = {:?}\nlaunch = {:?}\nusers = {{ {user} = {:?} }}\n",
        own.socket_path(),
        launch,
        other.socket_path()
    );
    let node = Node::start(&[parvi_terminals::LISTING], &config)
        .await
        .unwrap();
    let addr = node.address();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    // The session list comes before the launcher on connect, and it already
    // holds both servers' panes when the engines synced first.
    let mut launcher = None;
    let mut both_listed = false;
    until(&mut ws, "the launcher and both servers' sessions", |f| {
        let json = std::str::from_utf8(&f[1..]).unwrap();
        match f[0] {
            protocol::LAUNCHER => launcher = Some(json.to_owned()),
            protocol::SESSIONS => {
                both_listed |= json.contains(r#"{"id":"$0","name":"mine","panes":[{"#)
                    && json.contains(&format!(
                        r#"{{"id":"$0@{user}","name":"theirs","user":"{user}","panes":[{{"#
                    ))
            }
            _ => {}
        }
        launcher.is_some() && both_listed
    })
    .await;
    assert!(
        launcher
            .unwrap()
            .contains(&format!("\"users\":[\"{user}\"]"))
    );
    let theirs = listed(&node)
        .await
        .into_iter()
        .find(|s| s["name"] == "theirs")
        .unwrap();
    let pane = theirs["panes"][0]["id"].as_u64().unwrap() as u32;
    assert_eq!(pane >> 24, 1, "the other server's pane carries its index");

    // Typing into their pane reaches their server.
    ws.send(Message::Binary(
        protocol::encode_up(&Up::Subscribe(pane)).into(),
    ))
    .await
    .unwrap();
    until(&mut ws, "their pane's screen", |f| {
        f[0] == protocol::SCREEN && pane_of(f) == pane
    })
    .await;
    ws.send(Message::Binary(
        protocol::encode_up(&Up::Input(pane, b"echo hub-typed\r".to_vec())).into(),
    ))
    .await
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !other
        .run(&["capture-pane", "-p", "-t", "theirs"])
        .lines()
        .any(|l| l.ends_with("hub-typed") && !l.contains("echo"))
    {
        assert!(Instant::now() < deadline, "the input reached their pane");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // A search finds it under the hub's id for their pane.
    ws.send(Message::Binary(
        protocol::encode_up(&Up::Search {
            id: 3,
            query: "hub-typed".into(),
        })
        .into(),
    ))
    .await
    .unwrap();
    let found = until(&mut ws, "a match", |f| f[0] == protocol::MATCH).await;
    assert_eq!(u32::from_le_bytes(found[5..9].try_into().unwrap()), pane);

    // A launch for them lands on their server only.
    let launch = protocol::Launch {
        harness: "shell".into(),
        folder: "/tmp".into(),
        name: "fresh".into(),
        flags: String::new(),
        user: user.clone(),
        notify: false,
        size: None,
    };
    ws.send(Message::Binary(
        protocol::encode_up(&Up::Launch { id: 9, launch }).into(),
    ))
    .await
    .unwrap();
    let done = until(&mut ws, "the launch answered", |f| f[0] == protocol::DONE).await;
    assert_eq!(
        &done[1..6],
        &[9, 0, 0, 0, 1],
        "{:?}",
        String::from_utf8_lossy(&done[6..])
    );
    let session = String::from_utf8(done[6..].to_vec()).unwrap();
    assert_eq!(session, format!("$1@{user}"));
    let names = |tmux: &Tmux| tmux.run(&["list-sessions", "-F", "#{session_name}"]);
    assert_eq!(
        names(&other).lines().collect::<Vec<_>>(),
        ["fresh", "theirs"]
    );
    assert_eq!(names(&own).trim(), "mine");

    // Renaming it by the hub's id (#184) renames it there, even though the
    // service user's server has a session of that name.
    ws.send(Message::Binary(
        protocol::encode_up(&Up::Rename {
            id: 11,
            session: session.clone(),
            name: "mine".into(),
        })
        .into(),
    ))
    .await
    .unwrap();
    let done = until(&mut ws, "the rename answered", |f| f[0] == protocol::DONE).await;
    assert_eq!(
        &done[1..6],
        &[11, 0, 0, 0, 1],
        "{:?}",
        String::from_utf8_lossy(&done[6..])
    );
    assert_eq!(
        names(&other).lines().collect::<Vec<_>>(),
        ["mine", "theirs"]
    );
    assert_eq!(names(&own).trim(), "mine");
    assert!(
        listed(&node)
            .await
            .iter()
            .any(|s| s["id"] == session.as_str() && s["name"] == "mine"),
        "listed under the new name before the answer"
    );

    // The mouse toggle (#184) reaches every server the hub follows.
    for (id, on, word) in [(12, true, "on"), (13, false, "off")] {
        ws.send(Message::Binary(
            protocol::encode_up(&Up::Mouse { id, on }).into(),
        ))
        .await
        .unwrap();
        let done = until(&mut ws, "the mouse answered", |f| f[0] == protocol::DONE).await;
        assert_eq!(
            (&done[1..6], String::from_utf8_lossy(&done[6..])),
            (
                &[id as u8, 0, 0, 0, 1][..],
                format!("mouse {word}, on 2 of 2 tmux servers").into()
            )
        );
        for tmux in [&own, &other] {
            assert_eq!(tmux.run(&["show-options", "-gv", "mouse"]).trim(), word);
        }
    }

    // Deleting it by the hub's id kills it there.
    ws.send(Message::Binary(
        protocol::encode_up(&Up::Delete { id: 10, session }).into(),
    ))
    .await
    .unwrap();
    let done = until(&mut ws, "the delete answered", |f| f[0] == protocol::DONE).await;
    assert_eq!(&done[1..6], &[10, 0, 0, 0, 1]);
    assert_eq!(names(&other).trim(), "theirs");
    assert_eq!(names(&own).trim(), "mine");

    drop(ws);
    node.stop().await;
}
