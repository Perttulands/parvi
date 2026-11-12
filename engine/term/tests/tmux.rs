//! Tests against a real tmux on scratch sockets. Linux and macOS only.
//!
//! Without tmux these tests skip, unless `PARVI_REQUIRE_TMUX=1` (set in CI),
//! where a missing tmux fails them.
#![cfg(unix)]

use std::time::Duration;

use parvi_term::engine::{Engine, PaneUpdate, Sent};
use parvi_term::search::Target;
use parvi_testkit::tmux::*;

#[tokio::test(flavor = "multi_thread")]
async fn broken_utf8_before_clear_matches_tmux() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 80, 10);
    let engine = Engine::start(server.socket());
    eventually("alpha seeded", || {
        all_synced(&engine.snapshot(), &["alpha"]).then_some(())
    })
    .await;
    for (number, broken) in ["\\303", "\\343\\303"].into_iter().enumerate() {
        let marker = format!("clean-{number}");
        let command = format!("printf '{broken}\\033[H\\033[J{marker}\\n'");
        server.run(&["send-keys", "-t", "alpha", &command, "Enter"]);
        eventually("the final output to match tmux", || {
            let theirs = normalise(&capture(&server, "alpha"));
            let ours = normalise(&pane_text(&engine, "alpha")?);
            (theirs
                .lines()
                .any(|line| line.ends_with(&marker) && !line.contains("printf"))
                && ours == theirs)
                .then_some(())
        })
        .await;
    }
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_escape_after_incomplete_utf8_matches_tmux() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 80, 10);
    let engine = Engine::start(server.socket());
    eventually("alpha seeded", || {
        all_synced(&engine.snapshot(), &["alpha"]).then_some(())
    })
    .await;
    for (number, sequence) in [
        "\\033[31\\030",
        "\\033[31\\032",
        "\\033[31\\033]0;title\\007",
        "\\033[31\\033\\033[0m",
        "\\033]0;title\\030",
        "\\033]0;title\\033[0m",
        "\\033\\030",
        "\\033(\\032",
        "\\033P1\\030",
    ]
    .into_iter()
    .enumerate()
    {
        let marker = format!("cancel-{number}");
        let command = format!("printf '\\033[H\\033[J\\303{sequence}{marker}\\n'");
        server.run(&["send-keys", "-t", "alpha", &command, "Enter"]);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let theirs = normalise(&capture(&server, "alpha"));
            let ours = pane_text(&engine, "alpha").map(|text| normalise(&text));
            if theirs
                .lines()
                .any(|line| line.ends_with(&marker) && !line.contains("printf"))
                && ours.as_deref() == Some(theirs.as_str())
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{marker}: ours={ours:?}, tmux={theirs:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn holds_every_pane_and_matches_tmux() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 100, 30);
    server.new_session("beta", 80, 24);
    server.run(&[
        "send-keys",
        "-t",
        "alpha",
        "printf 'before %s\\n' start",
        "Enter",
    ]);

    let engine = Engine::start(server.socket());
    eventually("both sessions seeded", || {
        all_synced(&engine.snapshot(), &["alpha", "beta"]).then_some(())
    })
    .await;

    // Output after seeding arrives through %output.
    server.run(&[
        "send-keys",
        "-t",
        "alpha",
        "printf '\\033[1mafter\\033[0m %s\\n' seeding",
        "Enter",
    ]);
    server.run(&["send-keys", "-t", "beta", "seq 1 40", "Enter"]);
    for session in ["alpha", "beta"] {
        eventually(&format!("{session} to match capture-pane"), || {
            let ours = normalise(&pane_text(&engine, session)?);
            let theirs = normalise(&capture(&server, session));
            (ours == theirs).then_some(())
        })
        .await;
    }
    assert!(
        pane_text(&engine, "alpha")
            .unwrap()
            .contains("after seeding")
    );
    assert!(pane_text(&engine, "beta").unwrap().contains("40"));

    // Sessions come and go without a restart.
    server.new_session("gamma", 90, 20);
    server.run(&["kill-session", "-t", "beta"]);
    eventually("gamma to appear and beta to go", || {
        let snapshot = engine.snapshot();
        let names: Vec<&str> = snapshot.sessions.iter().map(|s| s.name.as_str()).collect();
        (all_synced(&snapshot, &["alpha", "gamma"]) && !names.contains(&"beta")).then_some(())
    })
    .await;

    // A split adds a pane, and both panes are followed.
    server.run(&["split-window", "-t", "alpha", "-h", "env PS1='$ ' sh"]);
    eventually("alpha's second pane", || {
        let snapshot = engine.snapshot();
        let alpha = snapshot.sessions.iter().find(|s| s.name == "alpha")?;
        (alpha.panes.len() == 2 && alpha.panes.iter().all(|p| p.synced)).then_some(())
    })
    .await;

    engine.stop().await;
}

/// A line tmux wrapped before the pane was first seen is still one line
/// once seeded, so a viewer selects and finds links across the wrap (#186).
#[tokio::test(flavor = "multi_thread")]
async fn a_line_wrapped_before_seeding_stays_one_line() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 20, 6);
    server.run(&["send-keys", "-t", "alpha", "printf '%030d\\n' 7", "Enter"]);
    eventually("the long line in tmux", || {
        capture(&server, "alpha")
            .contains("0000000007")
            .then_some(())
    })
    .await;
    let engine = Engine::start(server.socket());
    let text = eventually("alpha seeded", || pane_text(&engine, "alpha")).await;
    let long = format!("{:030}", 7);
    assert!(
        text.lines().any(|line| line.ends_with(&long)),
        "the wrapped line was split: {text:?}"
    );
    engine.stop().await;
}

/// Sessions coming and going while we attach and detach never take tmux
/// down. tmux 3.4 crashes if a client detaches while another is halfway
/// through identifying, which the in-process client's handshake avoids.
#[tokio::test(flavor = "multi_thread")]
async fn churn_never_takes_tmux_down() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("keep", 80, 24);
    let engine = Engine::start(server.socket());
    eventually("keep seeded", || {
        all_synced(&engine.snapshot(), &["keep"]).then_some(())
    })
    .await;
    for round in 0..40u64 {
        server.new_session(&format!("a{round}"), 80, 24);
        server.new_session(&format!("b{round}"), 80, 24);
        tokio::time::sleep(Duration::from_millis(round % 5 * 2)).await;
        server.run(&["kill-session", "-t", &format!("a{round}")]);
        server.run(&["kill-session", "-t", &format!("b{round}")]);
    }
    eventually("only keep left", || {
        let snapshot = engine.snapshot();
        (snapshot.sessions.len() == 1 && all_synced(&snapshot, &["keep"])).then_some(())
    })
    .await;
    engine.stop().await;
}

/// Issue #24: when tmux pauses a pane because we fell behind, we continue it
/// and start it over from a capture, which viewers get as a fresh screen.
/// Pausing it by hand stands in for falling behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_paused_pane_is_continued_and_reseeded() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 80, 24);
    let engine = Engine::start(server.socket());
    let pane = eventually("alpha seeded", || {
        let snapshot = engine.snapshot();
        let pane = snapshot.sessions.first()?.panes.first()?.clone();
        pane.synced.then_some(pane.id)
    })
    .await;
    let mut stream = engine.subscribe(pane).unwrap();
    assert!(stream.screen.is_some());

    let client = server.run(&["list-clients", "-F", "#{client_name}"]);
    server.run(&[
        "refresh-client",
        "-t",
        client.trim(),
        "-A",
        &format!("%{pane}:pause"),
    ]);
    let reseeded = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let PaneUpdate::Screen { .. } = stream.updates.recv().await.unwrap() {
                break;
            }
        }
    })
    .await;
    assert!(reseeded.is_ok(), "no fresh screen after the pause");

    // Output flows again, so the pane was continued.
    server.run(&["send-keys", "-t", "alpha", "seq 1 30", "Enter"]);
    eventually("alpha to match capture-pane after the pause", || {
        let ours = normalise(&pane_text(&engine, "alpha")?);
        let theirs = normalise(&capture(&server, "alpha"));
        (ours == theirs && ours.contains("30")).then_some(())
    })
    .await;
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_reaches_the_pane() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 100, 30);
    let engine = Engine::start(server.socket());
    let pane = eventually("alpha seeded", || {
        let snapshot = engine.snapshot();
        let pane = snapshot.sessions.first()?.panes.first()?.clone();
        pane.synced.then_some(pane.id)
    })
    .await;
    engine
        .send_input(pane, b"echo typed-$((40+2))\r".to_vec())
        .await
        .unwrap();
    eventually("typed output", || {
        capture(&server, "alpha").contains("typed-42").then_some(())
    })
    .await;
    eventually("typed output in our screen", || {
        pane_text(&engine, "alpha")?
            .contains("typed-42")
            .then_some(())
    })
    .await;
    engine.stop().await;
}

/// Issue #4, check 1: following a session nobody is viewing leaves its size
/// and options exactly as they were, during and after.
#[tokio::test(flavor = "multi_thread")]
async fn leaves_unviewed_sessions_alone() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("wide", 200, 50);
    server.new_session("default", 80, 24);
    server.new_session("odd", 133, 41);
    let sessions = ["wide", "default", "odd"];
    let before: Vec<String> = sessions.iter().map(|s| server.fingerprint(s)).collect();
    eprintln!("before: {before:#?}");
    eprintln!(
        "attached before: {}",
        server.display("wide", "#{session_attached} #{session_last_attached}")
    );

    let engine = Engine::start(server.socket());
    eventually("all seeded", || {
        all_synced(&engine.snapshot(), &sessions).then_some(())
    })
    .await;
    // Give tmux time to apply any resize it was going to make.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let during: Vec<String> = sessions.iter().map(|s| server.fingerprint(s)).collect();
    eprintln!("clients during: {:#?}", server.clients());
    eprintln!(
        "attached during: {}",
        server.display("wide", "#{session_attached} #{session_last_attached}")
    );
    assert_eq!(before, during, "following changed a session");
    for snapshot in engine.snapshot().sessions {
        let pane = &snapshot.panes[0];
        let size = format!("{}x{}", pane.cols, pane.rows);
        assert_eq!(size, server.window_size(&snapshot.name));
    }

    engine.stop().await;
    eventually("our clients to leave", || {
        server.clients().is_empty().then_some(())
    })
    .await;
    let after: Vec<String> = sessions.iter().map(|s| server.fingerprint(s)).collect();
    assert_eq!(before, after, "detaching changed a session");
}

/// Issue #4, checks 2, 3 and 5: a viewer keeps sizing its session while we
/// follow it, nobody is detached, and the size survives the viewer leaving.
#[tokio::test(flavor = "multi_thread")]
async fn leaves_a_viewers_size_and_attachment_alone() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 200, 50);
    let mut viewer = server.viewer("alpha", 120, 40);
    eventually("the viewer to size the window", || {
        (server.window_size("alpha") == "120x40").then_some(())
    })
    .await;
    assert_eq!(server.clients().len(), 1);

    let engine = Engine::start(server.socket());
    eventually("alpha seeded", || {
        all_synced(&engine.snapshot(), &["alpha"]).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let clients = server.clients();
    eprintln!("clients with viewer and follower: {clients:#?}");
    assert_eq!(
        clients.len(),
        2,
        "the viewer was detached or we attached twice"
    );
    assert_eq!(server.window_size("alpha"), "120x40");

    // The viewer still decides the size, and we follow it.
    viewer.resize(100, 30);
    eventually("the window to follow the viewer", || {
        (server.window_size("alpha") == "100x30").then_some(())
    })
    .await;
    eventually("our pane to follow the resize", || {
        let snapshot = engine.snapshot();
        let pane = snapshot.sessions.first()?.panes.first()?.clone();
        (pane.synced && (pane.cols, pane.rows) == (100, 30)).then_some(())
    })
    .await;

    // The viewer leaves; the size stays where it left it.
    viewer.detach();
    eventually("the viewer to leave", || {
        (server.clients().len() == 1).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(server.window_size("alpha"), "100x30");

    engine.stop().await;
    eventually("our client to leave", || {
        server.clients().is_empty().then_some(())
    })
    .await;
    assert_eq!(server.window_size("alpha"), "100x30");
}

/// What a pane's program read, through `cat -v` into a file, after a send.
async fn delivered_text(text: &str, enter: bool, bracketed: bool) -> String {
    let server = Server::start().expect("tmux is installed");
    let file = std::env::temp_dir().join(format!("{}-sent", server.name));
    let program = format!(
        "{}exec cat -v > {}",
        if bracketed {
            "printf '\\033[?2004h'; "
        } else {
            ""
        },
        file.display()
    );
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "agent",
        "-x",
        "100",
        "-y",
        "30",
        &program,
    ]);
    server.run(&["set-option", "-w", "-t", "agent", "remain-on-exit", "on"]);
    let engine = Engine::start(server.socket());
    let pane = seeded_pane(&engine).await;
    engine
        .deliver(pane, text.to_owned(), enter)
        .await
        .unwrap()
        .await
        .expect("tmux took the text");
    // Ctrl-D twice ends cat even after a line without a newline.
    engine.send_input(pane, b"\x04\x04".to_vec()).await.unwrap();
    eventually("cat to finish", || {
        (server.display("agent", "#{pane_dead}") == "1").then_some(())
    })
    .await;
    let read = std::fs::read_to_string(&file).expect("cat wrote the file");
    engine.stop().await;
    let _ = std::fs::remove_file(&file);
    read
}

/// Issue #19: multi-line text with characters tmux, the shell and JSON all
/// treat specially arrives exactly, as one paste, then Enter.
#[tokio::test(flavor = "multi_thread")]
async fn sent_text_arrives_intact() {
    if Server::start().is_none() {
        return;
    }
    let text = "fix \"this\" $HOME \\ back; #{pane_id} ~ é 💡\n  second line\nthird";
    assert_eq!(
        delivered_text(text, true, false).await,
        "fix \"this\" $HOME \\ back; #{pane_id} ~ M-CM-) M-pM-^_M-^RM-!\n  second line\nthird\n"
    );
    // A program that asked for bracketed paste gets it as one paste.
    assert_eq!(
        delivered_text("one\ntwo", true, true).await,
        "^[[200~one\ntwo^[[201~\n"
    );
    // Without Enter, nothing after the text.
    assert_eq!(delivered_text("no enter", false, false).await, "no enter");
}

#[tokio::test(flavor = "multi_thread")]
async fn sending_to_a_missing_pane_fails() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 80, 24);
    let engine = Engine::start(server.socket());
    seeded_pane(&engine).await;
    assert!(engine.deliver(99_999, "x".into(), true).await.is_err());
    engine.stop().await;
}

fn facts_of(engine: &Engine) -> Option<parvi_term::engine::Facts> {
    engine.sessions().first()?.facts.clone()
}

/// Issue #33: each badge condition set on a scratch session shows in its
/// facts, and output makes it active, then idle with the time it last wrote
/// and the program it now runs.
#[tokio::test(flavor = "multi_thread")]
async fn sees_each_badge_condition_and_activity() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 100, 30);
    // tmux 3.8 turns the mouse on by default.
    server.run(&["set-option", "-g", "mouse", "off"]);
    let engine = Engine::start(server.socket());
    let pane = seeded_pane(&engine).await;
    let plain = eventually("first facts", || facts_of(&engine)).await;
    assert_eq!(
        (
            plain.windows,
            plain.foreign_clients,
            plain.pinned,
            plain.mouse
        ),
        (1, 0, false, false)
    );
    assert_eq!(plain.command, "sh");
    assert!(!plain.path.is_empty(), "the pane's folder");

    server.run(&["new-window", "-d", "-t", "alpha"]);
    eventually("two windows", || {
        (facts_of(&engine)?.windows == 2).then_some(())
    })
    .await;
    // Options set by hand send no event; they show when a browser connects.
    server.run(&["set-option", "-w", "-t", "alpha", "window-size", "manual"]);
    engine.refresh_facts();
    eventually("pinned", || facts_of(&engine)?.pinned.then_some(())).await;
    server.run(&["set-option", "-t", "alpha", "mouse", "on"]);
    engine.refresh_facts();
    eventually("mouse on", || facts_of(&engine)?.mouse.then_some(())).await;
    let viewer = server.observer("alpha", 80, 24);
    eventually("a foreign client", || {
        (facts_of(&engine)?.foreign_clients == 1).then_some(())
    })
    .await;
    viewer.detach();
    eventually("no foreign client", || {
        (facts_of(&engine)?.foreign_clients == 0).then_some(())
    })
    .await;

    let before = std::time::SystemTime::now();
    engine
        .send_input(pane, b"echo busy; sleep 60\r".to_vec())
        .await
        .unwrap();
    eventually("active", || {
        engine.sessions()[0].activity.active.then_some(())
    })
    .await;
    let idle = eventually("idle again", || {
        let activity = engine.sessions()[0].activity;
        (!activity.active).then_some(activity)
    })
    .await;
    let wrote = idle.last_output.expect("the time it last wrote");
    assert!(
        wrote >= before - Duration::from_secs(1),
        "{wrote:?} is before the output"
    );
    assert!(
        wrote.elapsed().unwrap() >= Duration::from_secs(4),
        "idle only after a quiet spell"
    );
    // Going quiet reads the facts again: the program it now runs shows.
    eventually("the program it runs", || {
        (facts_of(&engine)?.command == "sleep").then_some(())
    })
    .await;
    engine.stop().await;
}

/// The flags tmux shows for the client that isn't ours.
#[tokio::test(flavor = "multi_thread")]
async fn thumbnails_send_each_first_pane_once_per_change() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 60, 10);
    server.new_session("beta", 40, 8);
    server.run(&["split-window", "-t", "beta", "env PS1='$ ' sh"]);
    let engine = Engine::start(server.socket());
    eventually("both sessions seeded", || {
        all_synced(&engine.snapshot(), &["alpha", "beta"]).then_some(())
    })
    .await;

    // The first look sends every session's first pane, and only that one;
    // with a limit, the rest wait for the next look.
    let mut limited = Sent::default();
    let one = engine.thumbnails(&mut limited, 1);
    let other = engine.thumbnails(&mut limited, 1);
    assert_eq!(one.len() + other.len(), 2);
    assert_ne!(one[0].pane, other[0].pane);
    let mut seen = Sent::default();
    let first = engine.thumbnails(&mut seen, 50);
    let snapshot = engine.snapshot();
    let first_panes: Vec<u32> = snapshot.sessions.iter().map(|s| s.panes[0].id).collect();
    assert_eq!(
        first.iter().map(|t| t.pane).collect::<Vec<_>>(),
        first_panes
    );
    let alpha = first_panes[0];
    assert_eq!((first[0].cols, first[0].rows), (60, 10));
    assert_eq!(first[0].text.split('\n').count(), 10);

    // Nothing changed, nothing sent.
    assert!(engine.thumbnails(&mut seen, 50).is_empty());

    server.run(&["send-keys", "-t", "alpha", "echo thumb-marker", "Enter"]);
    let changed = eventually("alpha's change", || {
        let fresh = engine.thumbnails(&mut seen, 50);
        fresh
            .iter()
            .any(|t| t.text.contains("thumb-marker"))
            .then_some(fresh)
    })
    .await;
    assert!(changed.iter().all(|t| t.pane == alpha));

    // A session that goes is forgotten.
    server.run(&["kill-session", "-t", "alpha"]);
    eventually("alpha forgotten", || {
        engine.thumbnails(&mut seen, 50);
        (!seen.has(alpha)).then_some(())
    })
    .await;
    assert_eq!(seen.len(), 1);
}
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_a_line_that_scrolled_off_screen() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("old", 80, 10);
    server.new_session("other", 80, 10);
    server.run(&[
        "send-keys",
        "-t",
        "old",
        "echo Scrolled-Marker; seq 1 300",
        "Enter",
    ]);
    let history = || server.display("old", "#{history_size}");
    eventually("the marker to scroll into history", || {
        (history().trim().parse::<u32>().ok()? > 250).then_some(())
    })
    .await;
    let pane: u32 = server
        .display("old", "#{pane_id}")
        .trim()
        .trim_start_matches('%')
        .parse()
        .unwrap();
    let other: u32 = server
        .display("other", "#{pane_id}")
        .trim()
        .trim_start_matches('%')
        .parse()
        .unwrap();

    let (found, mut matches) = tokio::sync::mpsc::channel(16);
    let socket = server.socket();
    let targets = [pane, other, 9999]
        .map(|p| Target::own(&socket, p))
        .to_vec();
    let summary = parvi_term::search::search(targets, "scrolled-marker", found).await;
    let mut all = Vec::new();
    while let Ok(m) = matches.try_recv() {
        all.push(m);
    }
    // The typed command line and its output both say it; the output line is
    // the one followed by seq's first number. Keys typed before the shell's
    // first prompt leave that prompt in front of the output.
    let output = all
        .iter()
        .find(|m| m.context[1].ends_with("Scrolled-Marker") && m.context[2] == "1")
        .expect("the marker's output line");
    assert_eq!(output.pane, pane);
    let history: u32 = history().trim().parse().unwrap();
    assert!(
        output.line < history,
        "line {} is in history ({history})",
        output.line
    );
    // The missing pane is skipped; the other two were read.
    assert_eq!(summary.panes, 2);
    assert!(!summary.truncated);
    assert_eq!(summary.matches as usize, all.len());

    // The escaped capture has the same rows, so the line finds it there.
    let escaped = parvi_term::search::capture(&server.socket(), pane)
        .await
        .unwrap();
    let row = escaped.lines().nth(output.line as usize).unwrap();
    assert!(row.contains("Scrolled-Marker"), "{row:?}");

    // A capital letter makes case matter.
    let (found, _matches) = tokio::sync::mpsc::channel(16);
    let summary =
        parvi_term::search::search(vec![Target::own(&socket, pane)], "SCROLLED", found).await;
    assert_eq!(summary.matches, 0);
}
fn viewer_flags(server: &Server) -> String {
    server
        .clients()
        .into_iter()
        .find(|client| !client.contains("pause-after"))
        .expect("the viewer is attached")
}

fn our_flags(server: &Server) -> String {
    server
        .clients()
        .into_iter()
        .find(|client| client.contains("pause-after"))
        .expect("our client is attached")
}

async fn seeded_pane(engine: &Engine) -> u32 {
    eventually("the pane seeded", || {
        let snapshot = engine.snapshot();
        let pane = snapshot.sessions.first()?.panes.first()?.clone();
        pane.synced.then_some(pane.id)
    })
    .await
}

/// Issue #15: a claim moves the sizing seat to us without detaching anyone,
/// holds it against the viewer, follows the browser's grid, passes to another
/// holder, and goes back to the viewer on release.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_takes_the_seat_and_a_release_gives_it_back() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 200, 50);
    let mut viewer = server.viewer("alpha", 120, 40);
    window_becomes(&server, "alpha", "120x40").await;
    let engine = Engine::start(server.socket());
    let pane = seeded_pane(&engine).await;

    engine.claim(pane, 1, 90, 30).await.unwrap();
    window_becomes(&server, "alpha", "90x30").await;
    assert_eq!(server.clients().len(), 2, "nobody is detached");
    assert!(viewer_flags(&server).contains("ignore-size"));
    assert!(!our_flags(&server).contains("ignore-size"));
    assert_eq!(engine.sessions()[0].seat, Some(1));

    // The viewer no longer sizes the window; the browser's grid does.
    viewer.resize(130, 45);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.window_size("alpha"), "90x30");
    engine.claim(pane, 1, 100, 35).await.unwrap();
    window_becomes(&server, "alpha", "100x35").await;
    eventually("our pane at the claimed grid", || {
        let pane = engine.snapshot().sessions.first()?.panes.first()?.clone();
        (pane.synced && (pane.cols, pane.rows) == (100, 35)).then_some(())
    })
    .await;

    // Another browser takes the seat; the first can no longer release it.
    engine.claim(pane, 2, 80, 24).await.unwrap();
    window_becomes(&server, "alpha", "80x24").await;
    assert_eq!(engine.sessions()[0].seat, Some(2));
    engine.release(pane, 1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.window_size("alpha"), "80x24");

    // Released: the viewer sizes again, at the size it asked for meanwhile.
    engine.release(pane, 2).await.unwrap();
    // At once, not at tmux's next resize check seconds later.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.window_size("alpha"), "130x45");
    assert_eq!(engine.sessions()[0].seat, None);
    assert!(!viewer_flags(&server).contains("ignore-size"));
    assert!(our_flags(&server).contains("ignore-size"));
    assert_eq!(server.clients().len(), 2);
    engine.stop().await;
}

/// With nobody else sizing, a release leaves the claimed size; and stopping
/// while holding the seat gives it back before our client leaves.
#[tokio::test(flavor = "multi_thread")]
async fn a_release_with_nobody_else_keeps_the_size_and_stopping_releases() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 200, 50);
    let engine = Engine::start(server.socket());
    let pane = seeded_pane(&engine).await;
    engine.claim(pane, 1, 90, 30).await.unwrap();
    window_becomes(&server, "alpha", "90x30").await;
    engine.release(pane, 1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.window_size("alpha"), "90x30");
    assert!(our_flags(&server).contains("ignore-size"));

    let viewer = server.viewer("alpha", 120, 40);
    window_becomes(&server, "alpha", "120x40").await;
    engine.claim(pane, 1, 100, 35).await.unwrap();
    window_becomes(&server, "alpha", "100x35").await;
    engine.stop().await;
    eventually("our client to leave", || {
        (server.clients().len() == 1).then_some(())
    })
    .await;
    assert!(!viewer_flags(&server).contains("ignore-size"));
    window_becomes(&server, "alpha", "120x40").await;
    viewer.detach();
}

/// Issue #4, check 4: with a sizing viewer and an `ignore-size` observer, the
/// window goes through exactly the same sizes whether we follow it or not. So
/// we never take, hold or block the sizing seat.
#[tokio::test(flavor = "multi_thread")]
async fn the_sizing_seat_behaves_as_if_we_were_not_there() {
    let Some(server) = Server::start() else {
        return;
    };
    let without = seat_scenario(&server, "without", false).await;
    let with = seat_scenario(&server, "with", true).await;
    eprintln!("sizes without us: {without:?}, with us: {with:?}");
    assert_eq!(without, with);
}

/// Viewer sizes, observer joins, viewer resizes, viewer leaves, observer
/// leaves. Returns the window size after each step.
async fn seat_scenario(server: &Server, session: &str, follow: bool) -> Vec<String> {
    server.new_session(session, 200, 50);
    let engine = follow.then(|| Engine::start(server.socket()));
    if let Some(engine) = &engine {
        eventually("seeded", || {
            all_synced(&engine.snapshot(), &[session]).then_some(())
        })
        .await;
    }
    let settle = || tokio::time::sleep(Duration::from_millis(300));
    let mut sizes = Vec::new();

    let mut viewer = server.viewer(session, 120, 40);
    settle().await;
    sizes.push(server.window_size(session));
    let observer = server.observer(session, 90, 20);
    settle().await;
    sizes.push(server.window_size(session));
    viewer.resize(110, 35);
    settle().await;
    sizes.push(server.window_size(session));
    viewer.detach();
    settle().await;
    sizes.push(server.window_size(session));
    observer.detach();
    settle().await;
    sizes.push(server.window_size(session));

    if let Some(engine) = engine {
        engine.stop().await;
    }
    server.run(&["kill-session", "-t", session]);
    sizes
}

/// Issue #4, check 6: side effects other tools can see, recorded rather than
/// asserted. On tmux 3.4: `session_attached` goes to 1, the client hooks fire,
/// and a focus-in reaches the active pane of the session's current window.
#[tokio::test(flavor = "multi_thread")]
async fn records_what_other_tools_can_see() {
    let Some(server) = Server::start() else {
        return;
    };
    let seen = follow_with_hooks_and_focus(&server).await;
    eprintln!(
        "FINDING session_attached: before={} during={}",
        seen.attached_before, seen.attached_during
    );
    eprintln!("FINDING hooks fired: {:?}", seen.hooks);
    eprintln!(
        "FINDING focus events the pane saw: {:?}",
        seen.focus_events()
    );
}

/// The goal from issue #4: following must not send focus events, because an
/// agent can read a focus-in as the operator arriving. tmux 3.4 marks every
/// client focused on attach and only a terminal's focus report clears it, so
/// a control client can't avoid this. Run with `--ignored` to check a newer
/// tmux.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "fails on tmux 3.4: attaching focuses the active pane (issue #4)"]
async fn never_sends_focus_events() {
    let Some(server) = Server::start() else {
        return;
    };
    let seen = follow_with_hooks_and_focus(&server).await;
    assert_eq!(seen.focus_events(), Vec::<&str>::new());
}

struct Seen {
    attached_before: String,
    attached_during: String,
    hooks: Vec<String>,
    screen: String,
}

impl Seen {
    fn focus_events(&self) -> Vec<&str> {
        let mut events = Vec::new();
        for line in self.screen.lines() {
            let mut rest = line;
            while let Some(i) = rest.find("^[[") {
                match rest[i + 3..].chars().next() {
                    Some('I') => events.push("focus-in"),
                    Some('O') => events.push("focus-out"),
                    _ => {}
                }
                rest = &rest[i + 3..];
            }
        }
        events
    }
}

/// Follows a session whose pane asks for focus reports, with every client hook
/// logging to a file, then stops following and reports what was seen.
async fn follow_with_hooks_and_focus(server: &Server) -> Seen {
    let log = std::env::temp_dir().join(format!("{}-hooks", server.name));
    let _ = std::fs::remove_file(&log);
    server.new_session("alpha", 100, 30);
    server.run(&["set-option", "-g", "focus-events", "on"]);
    for hook in [
        "client-attached",
        "client-detached",
        "client-session-changed",
        "client-focus-in",
        "client-focus-out",
    ] {
        server.run(&[
            "set-hook",
            "-g",
            hook,
            &format!("run-shell 'echo {hook} >> {}'", log.display()),
        ]);
    }
    // A program that asks for focus reports; the tty echoes them as ^[[I/^[[O.
    server.run(&[
        "send-keys",
        "-t",
        "alpha",
        "printf '\\033[?1004h'; cat -v",
        "Enter",
    ]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let attached_before = server.display("alpha", "#{session_attached}");

    let engine = Engine::start(server.socket());
    eventually("seeded", || {
        all_synced(&engine.snapshot(), &["alpha"]).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let attached_during = server.display("alpha", "#{session_attached}");
    engine.stop().await;
    eventually("our client to leave", || {
        server.clients().is_empty().then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let hooks = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let _ = std::fs::remove_file(&log);
    Seen {
        attached_before,
        attached_during,
        hooks,
        screen: capture(server, "alpha"),
    }
}

/// The fake prompt: Claude Code's shape, then a clear screen once answered.
const PROMPT: &str = "clear; printf 'Do you want to proceed?\\n\\342\\235\\257 1. Yes\\n  2. No\\n\\nEsc to cancel\\n'; read answer; clear; echo answered-$answer";

fn waiting_in(engine: &Engine, name: &str) -> Option<Vec<(u32, String)>> {
    let sessions = engine.sessions();
    Some(sessions.into_iter().find(|s| s.name == name)?.waiting)
}

#[tokio::test(flavor = "multi_thread")]
async fn sees_a_prompt_wait_and_clear() {
    let Some(server) = Server::start() else {
        return;
    };
    // One prompt is up before we start, one comes after.
    server.new_session("early", 80, 24);
    server.new_session("late", 80, 24);
    server.run(&["send-keys", "-t", "early", PROMPT, "Enter"]);
    eventually("the early prompt on screen", || {
        capture(&server, "early")
            .contains("Esc to cancel")
            .then_some(())
    })
    .await;

    let engine = Engine::start(server.socket());
    eventually("both seeded", || {
        all_synced(&engine.snapshot(), &["early", "late"]).then_some(())
    })
    .await;
    let early = eventually("the early prompt waits", || {
        let waiting = waiting_in(&engine, "early")?;
        (!waiting.is_empty()).then_some(waiting)
    })
    .await;
    assert_eq!(early[0].1, "Do you want to proceed?");
    assert_eq!(waiting_in(&engine, "late"), Some(vec![]));

    server.run(&["send-keys", "-t", "late", PROMPT, "Enter"]);
    eventually("the late prompt waits", || {
        (!waiting_in(&engine, "late")?.is_empty()).then_some(())
    })
    .await;

    // Answered, the screen moves on and the wait clears.
    server.run(&["send-keys", "-t", "late", "1", "Enter"]);
    eventually("the late prompt clears", || {
        waiting_in(&engine, "late")?.is_empty().then_some(())
    })
    .await;
    assert!(!waiting_in(&engine, "early").unwrap().is_empty());
    engine.stop().await;
}

/// A streamed command that tmux refuses says why, as other commands do.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_stream_reports_what_tmux_said() {
    use parvi_term::backend::Backend;
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 80, 10);
    let socket = server.socket();
    let mut stream = socket
        .stream(&["capture-pane", "-p", "-t", "%999"])
        .await
        .unwrap();
    let error = (&mut stream.completion).await.unwrap().unwrap_err();
    assert!(error.to_string().contains("can't find pane"), "{error:#}");
}

/// Right after start the engine has not listed anything; once ready, every
/// session the server held is there with its panes (parvi-iay).
#[tokio::test(flavor = "multi_thread")]
async fn ready_lists_every_session_with_its_panes() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("alpha", 80, 24);
    server.new_session("beta", 80, 24);
    let engine = Engine::start(server.socket());
    engine.ready().await;
    let mut listed: Vec<_> = engine
        .sessions()
        .into_iter()
        .map(|s| (s.name, s.panes.len()))
        .collect();
    listed.sort();
    assert_eq!(listed, [("alpha".to_string(), 1), ("beta".to_string(), 1)]);
    engine.stop().await;
}

/// With no server running, the engine is ready with no sessions.
#[tokio::test(flavor = "multi_thread")]
async fn ready_with_no_server_lists_nothing() {
    let Some(server) = Server::start() else {
        return;
    };
    let engine = Engine::start(server.socket());
    engine.ready().await;
    assert!(engine.sessions().is_empty());
    engine.stop().await;
}

/// Other Parvi followers use control mode with pause-after; they do not own a
/// sizing seat. A viewer that actually sizes the session still counts.
#[tokio::test(flavor = "multi_thread")]
async fn another_follower_is_not_a_foreign_viewer() {
    let Some(server) = Server::start_in("terminal-other-follower") else {
        return;
    };
    server.new_session("alpha", 200, 50);
    let first = Engine::start(server.socket());
    let second = Engine::start(server.socket());
    seeded_pane(&first).await;
    seeded_pane(&second).await;
    eventually("both followers attached", || {
        (server.clients().len() == 2).then_some(())
    })
    .await;
    eventually("followers excluded from foreign clients", || {
        first
            .sessions()
            .first()?
            .facts
            .as_ref()
            .filter(|f| f.foreign_clients == 0)
            .map(|_| ())
    })
    .await;
    use std::os::fd::FromRawFd;
    struct Viewer(std::process::Child, std::fs::File);
    impl Drop for Viewer {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let (mut master, mut slave) = (-1, -1);
    let size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty initializes both descriptors, and each is owned once below.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        },
        0
    );
    // SAFETY: the successful openpty call returned fresh owned descriptors.
    let master = unsafe { std::fs::File::from_raw_fd(master) };
    // SAFETY: the slave descriptor is separate from master and also freshly owned.
    let slave = unsafe { std::fs::File::from_raw_fd(slave) };
    let process = std::process::Command::new(parvi_term::backend::tmux::program())
        .args([
            "-S",
            server.socket_path().to_str().unwrap(),
            "attach-session",
            "-t",
            "alpha",
        ])
        .env("TERM", "xterm")
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave)
        .spawn()
        .unwrap();
    let viewer = Viewer(process, master);
    eventually("real terminal client attached", || {
        (server.clients().len() == 3).then_some(())
    })
    .await;
    eventually("real viewer counted", || {
        first
            .sessions()
            .first()?
            .facts
            .as_ref()
            .filter(|f| f.foreign_clients == 1)
            .map(|_| ())
    })
    .await;
    let _ = &viewer.1;
    drop(viewer);
    first.stop().await;
    second.stop().await;
}

/// Starting a follower on an idle session reads the pre-existing activity
/// timestamp; a later real write still marks it active.
#[tokio::test(flavor = "multi_thread")]
async fn following_idle_sessions_preserves_their_old_activity() {
    let Some(server) = Server::start_in("terminal-idle-activity") else {
        return;
    };
    server.new_session("alpha", 100, 30);
    server.new_session("beta", 100, 30);
    let before: std::collections::BTreeMap<String, u64> = ["alpha", "beta"]
        .into_iter()
        .map(|name| {
            (
                name.to_string(),
                server.display(name, "#{window_activity}").parse().unwrap(),
            )
        })
        .collect();
    let latest = *before.values().max().unwrap();
    while std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        <= latest + 1
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let engine = Engine::start(server.socket());
    eventually("idle sessions seeded", || {
        let sessions = engine.sessions();
        (sessions.len() == 2
            && sessions
                .iter()
                .all(|s| s.activity.last_output.is_some() && !s.panes.is_empty()))
        .then_some(())
    })
    .await;
    for session in engine.sessions() {
        let last = session
            .activity
            .last_output
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(
            last, before[&session.name],
            "{} acquired a new activity timestamp",
            session.name
        );
        assert!(
            !session.activity.active,
            "{} became active while followed",
            session.name
        );
    }
    server.run(&["send-keys", "-t", "alpha", "echo real-output", "Enter"]);
    eventually("real output marks active", || {
        engine
            .sessions()
            .iter()
            .find(|s| s.name == "alpha" && s.activity.active)
            .map(|_| ())
    })
    .await;
    engine.stop().await;
}

/// A program that repaints its whole screen when its window changes size,
/// as vim does, has not written anything new: the claim that resized it
/// leaves the session idle and its last output where it was. Writing later
/// is news again. Like Claude Code, it repaints all it has shown from the
/// top, so the taller window shows a banner that was above the screen. A
/// follower that starts later starts from the same last output.
#[tokio::test(flavor = "multi_thread")]
async fn a_repaint_for_a_new_size_is_not_activity() {
    let Some(server) = Server::start_in("terminal-repaint") else {
        return;
    };
    server.new_session("alpha", 100, 30);
    let script = server.dir().join("repaint.sh");
    write_script(
        &script,
        format!(
            "#!/bin/sh\npaint() {{ printf '\\033[H\\033[2Kbanner\\n'; seq 1 40; printf ready; }}\ntrap 'paint; touch {}' WINCH\npaint\nwhile :; do sleep 0.2; done\n",
            server.dir().join("repainted").display()
        ),
    );
    server.run(&[
        "respawn-pane",
        "-k",
        "-t",
        "alpha",
        script.to_str().unwrap(),
    ]);
    let wrote_at = || -> u64 {
        server
            .display("alpha", "#{window_activity}")
            .parse()
            .unwrap()
    };
    let before = wrote_at();
    while std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        <= before + 1
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let engine = Engine::start(server.socket());
    let pane = seeded_pane(&engine).await;
    let last = |engine: &Engine| engine.sessions()[0].activity.last_output;
    let idle = eventually("seeded idle", || last(&engine)).await;

    engine.claim(pane, 1, 100, 50).await.unwrap();
    eventually("the program repainted", || {
        server.dir().join("repainted").exists().then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let session = &engine.sessions()[0];
    assert!(!session.activity.active, "a repaint made it active");
    assert_eq!(
        session.activity.last_output,
        Some(idle),
        "a repaint moved its last output"
    );

    // tmux counted the repaint as activity. The next follower, as after a
    // node restarts, starts from the output this one kept as it left.
    let idle_secs = idle
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    engine.stop().await;
    assert!(wrote_at() > idle_secs, "tmux saw the repaint");
    let engine = Engine::start(server.socket());
    seeded_pane(&engine).await;
    let again = eventually("followed again", || last(&engine)).await;
    assert_eq!(again, idle, "a new follower took the repaint for output");

    // The same repaint with no new size is the program writing.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let pid = server.display("alpha", "#{pane_pid}");
    std::process::Command::new("kill")
        .args(["-WINCH", pid.trim()])
        .status()
        .unwrap();
    eventually("a later write is news", || {
        engine.sessions()[0].activity.active.then_some(())
    })
    .await;
    engine.stop().await;
}
