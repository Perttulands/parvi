#![cfg(unix)]
use parvi_term::{Engine, SessionSpec};
use parvi_testkit::tmux::{Server, capture, eventually};

fn spec(name: &str) -> SessionSpec {
    SessionSpec {
        name: name.to_owned(),
        cwd: std::env::temp_dir(),
        env: [("PARVI_OWNED_TEST".into(), "yes".into())].into(),
        command: vec![
            "sh".into(),
            "-c".into(),
            "printf '\\033[2mghost\\033[22mtyped %s' \"$PARVI_OWNED_TEST\"; exec sleep 120".into(),
        ],
        cols: 160,
        rows: 48,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_screen_pin_and_generation() {
    let Some(server) = Server::start() else {
        return;
    };
    let engine = Engine::start(server.socket());
    let owned = engine.create_session(spec("owned")).await.unwrap();
    engine.verify_session(&owned).await.unwrap();
    assert_eq!(
        engine.find_session("owned").await.unwrap(),
        Some(owned.clone())
    );
    assert_eq!(engine.find_session("own").await.unwrap(), None);
    let screen = eventually("styled screen", || {
        let screen = engine.read_screen(owned.pane_id)?;
        (screen.rows[0][0].text == "g"
            && screen.rows[0][5].text == "t"
            && screen.rows[0][11].text == "y")
            .then_some(screen)
    })
    .await;
    assert!(screen.rows[0][0].dim);
    assert!(!screen.rows[0][5].dim);
    let cursor = server.display(&owned.session_id, "#{cursor_x} #{cursor_y}");
    assert_eq!(
        cursor,
        format!("{} {}", screen.cursor.column, screen.cursor.row)
    );
    assert_eq!(server.window_size(&owned.session_id), "160x48");
    let error = engine.claim(owned.pane_id, 1, 90, 25).await.unwrap_err();
    assert!(error.to_string().contains("pinned"), "{error:#}");
    assert_eq!(server.window_size(&owned.session_id), "160x48");
    assert!(
        engine
            .sessions()
            .iter()
            .all(|session| session.seat.is_none())
    );
    engine.kill_session(&owned).await.unwrap();
    assert_eq!(engine.find_session("owned").await.unwrap(), None);
    engine.stop().await;
}

/// A mission seat looks for its session before creating one; on a node
/// whose tmux server has not started yet, or exited with its last session,
/// there is none to find.
#[tokio::test(flavor = "multi_thread")]
async fn finds_no_session_when_no_server_runs() {
    let Some(server) = Server::start() else {
        return;
    };
    let engine = Engine::start(server.socket());
    assert_eq!(engine.find_session("owned").await.unwrap(), None);
    // tmux leaves its socket file behind when the server exits.
    server.new_session("bootstrap", 80, 24);
    server.kill();
    assert_eq!(engine.find_session("owned").await.unwrap(), None);
    engine.stop().await;
}

/// A server the node cannot reach may hold the session, so creating another
/// would duplicate it: only an absent server means no session.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_server_is_an_error_not_no_session() {
    use std::os::unix::fs::PermissionsExt;
    // The path holds tmux's words for a missing session; they are not the reason.
    let Some(server) = Server::start_in("can't find session: owned") else {
        return;
    };
    server.new_session("owned", 80, 24);
    let dir = server.socket().path().parent().unwrap().to_owned();
    let engine = Engine::start(server.socket());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let found = engine.find_session("owned").await;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let error = found.unwrap_err();
    assert!(error.to_string().contains("Permission denied"), "{error:#}");
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_to_kill_a_replacement_server() {
    let Some(server) = Server::start() else {
        return;
    };
    let engine = Engine::start(server.socket());
    let old = engine.create_session(spec("old")).await.unwrap();
    server.kill();
    server.new_session("replacement", 80, 24);
    assert_eq!(
        old.session_id,
        server.display("replacement", "#{session_id}")
    );
    let error = engine.kill_session(&old).await.unwrap_err();
    assert!(error.to_string().contains("generation"), "{error:#}");
    assert!(engine.verify_session(&old).await.is_err());
    assert_eq!(
        server.display("replacement", "#{session_name}"),
        "replacement"
    );
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn seeded_cursor_visibility_matches_tmux() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("hidden", 80, 24);
    server.run(&["send-keys", "-t", "hidden", "printf '\\033[?25l'", "Enter"]);
    eventually("tmux hides its cursor", || {
        (server.display("hidden", "#{cursor_flag}") == "0").then_some(())
    })
    .await;
    let pane: u32 = server
        .display("hidden", "#{pane_id}")
        .trim_start_matches('%')
        .parse()
        .unwrap();
    let engine = Engine::start(server.socket());
    let screen = eventually("hidden cursor seeded", || engine.read_screen(pane)).await;
    assert!(!screen.cursor.visible);
    server.run(&["send-keys", "-t", "hidden", "printf '\\033[?25h'", "Enter"]);
    eventually("cursor shown", || {
        engine.read_screen(pane)?.cursor.visible.then_some(())
    })
    .await;
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn output_keeps_active_style_after_attach_and_resize_reseed() {
    use parvi_term::engine::PaneUpdate;
    use std::time::Duration;

    let Some(server) = Server::start() else {
        return;
    };
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "styled",
        "-x",
        "80",
        "-y",
        "24",
        "stty -echo; printf '\\033[2mBEFORE'; read x; printf AFTER; read x; printf THIRD; sleep 60",
    ]);
    eventually("first styled text in tmux", || {
        capture(&server, "styled").contains("BEFORE").then_some(())
    })
    .await;
    let pane: u32 = server
        .display("styled", "#{pane_id}")
        .trim_start_matches('%')
        .parse()
        .unwrap();
    let engine = Engine::start(server.socket());
    eventually("styled screen seeded", || {
        engine
            .read_screen(pane)
            .filter(|s| s.rows[0][0].text == "B")
    })
    .await;
    let mut stream = engine.subscribe(pane).unwrap();
    let Some(PaneUpdate::Screen { cols, rows, bytes }) = stream.screen.take() else {
        panic!("seeded screen")
    };
    let mut viewer = vt100::Parser::new(rows, cols, 0);
    viewer.process(&bytes);
    assert!(viewer.screen().dim(), "viewer seed lost active dim style");
    engine.send_input(pane, b"\r".to_vec()).await.unwrap();
    let screen = eventually("output after attach", || {
        engine
            .read_screen(pane)
            .filter(|s| s.rows[0][6].text == "A")
    })
    .await;
    assert!(
        screen.rows[0][6].dim,
        "output after attachment lost dim style"
    );
    server.run(&["set-option", "-w", "-t", "styled", "window-size", "manual"]);
    server.run(&["resize-window", "-t", "styled", "-x", "100", "-y", "30"]);
    let (cols, rows, bytes) = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let PaneUpdate::Screen {
                cols: 100,
                rows: 30,
                bytes,
            } = stream.updates.recv().await.unwrap()
            {
                break (100, 30, bytes);
            }
        }
    })
    .await
    .expect("resize reseed");
    let mut viewer = vt100::Parser::new(rows, cols, 0);
    viewer.process(&bytes);
    assert!(viewer.screen().dim(), "viewer reseed lost active dim style");
    engine.send_input(pane, b"\r".to_vec()).await.unwrap();
    let screen = eventually("output after resize reseed", || {
        engine
            .read_screen(pane)
            .filter(|s| s.rows[0][11].text == "T")
    })
    .await;
    assert!(
        screen.rows[0][11].dim,
        "output after reseeding lost dim style"
    );
    engine.stop().await;
}
