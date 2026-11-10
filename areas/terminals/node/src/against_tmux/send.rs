//! Send's timing against a real tmux (issue #63): Enter waits for the paste
//! to settle, goes only to the process the text went to, and a session's
//! sends never interleave. A send into a just-launched pane waits until the
//! pane is ready for a paste (#242).
//!
//! `agent_seats_submit` drives a real Claude Code or Codex seat and needs a
//! signed-in harness, so it runs only when asked:
//! `PARVI_AGENT_SEATS=claude,codex cargo test -p parvi-terminals agent_seats_submit`.
//! Linux and macOS only, as tmux is.

use std::time::{Duration, Instant};

use parvi_term::engine::{Engine, NotReady, NotSubmitted};
use parvi_testkit::fresh_dir;
use parvi_testkit::tmux::{Server, eventually, write_script};

/// The pane of the only session, once seeded.
async fn seeded_pane(engine: &Engine) -> u32 {
    eventually("the pane seeded", || {
        let snapshot = engine.snapshot();
        let pane = snapshot.sessions.first()?.panes.first()?.clone();
        pane.synced.then_some(pane.id)
    })
    .await
}

/// A session `agent` whose program writes what it reads, `cat -v`, to `file`.
fn cat_session(server: &Server, file: &std::path::Path) {
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "agent",
        "-x",
        "100",
        "-y",
        "30",
        &format!("exec cat -v > {}", file.display()),
    ]);
    server.run(&["set-option", "-w", "-t", "agent", "remain-on-exit", "on"]);
}

/// Ends the pane's `cat` and returns what it read.
async fn read_back(server: &Server, engine: &Engine, pane: u32, file: &std::path::Path) -> String {
    // Ctrl-D twice ends cat even after a line without a newline.
    engine.send_input(pane, b"\x04\x04".to_vec()).await.unwrap();
    eventually("cat to finish", || {
        (server.display("agent", "#{pane_dead}") == "1").then_some(())
    })
    .await;
    let read = std::fs::read_to_string(file).expect("cat wrote the file");
    let _ = std::fs::remove_file(file);
    read
}

fn scratch_file(server: &Server, what: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("{}-63-{what}", server.name))
}

#[tokio::test(flavor = "multi_thread")]
async fn enter_waits_for_the_paste_to_settle() {
    let Some(server) = Server::start() else {
        return;
    };
    let file = scratch_file(&server, "settle");
    cat_session(&server, &file);
    let engine = Engine::start(server.socket());
    let pane = seeded_pane(&engine).await;
    let started = Instant::now();
    let delivered = engine.deliver(pane, "hello".into(), true).await.unwrap();
    // The text is on the screen, echoed by the terminal, long before Enter.
    eventually("the paste", || {
        server
            .run(&["capture-pane", "-p", "-t", "agent"])
            .contains("hello")
            .then_some(())
    })
    .await;
    delivered.await.expect("tmux took the text and the Enter");
    assert!(
        started.elapsed() >= Duration::from_millis(1200),
        "Enter came {:?} after the send",
        started.elapsed()
    );
    assert_eq!(read_back(&server, &engine, pane, &file).await, "hello\n");
    engine.stop().await;
}

/// A second send waits for the first one's Enter, so neither paste lands
/// inside the other's prompt.
#[tokio::test(flavor = "multi_thread")]
async fn sends_to_one_session_never_interleave() {
    let Some(server) = Server::start() else {
        return;
    };
    let file = scratch_file(&server, "order");
    cat_session(&server, &file);
    let engine = Engine::start(server.socket());
    let pane = seeded_pane(&engine).await;
    let first = engine.deliver(pane, "first".into(), true).await.unwrap();
    let second = engine.deliver(pane, "second".into(), true).await.unwrap();
    let (first, second) = tokio::join!(first, second);
    first.expect("first delivered");
    second.expect("second delivered");
    assert_eq!(
        read_back(&server, &engine, pane, &file).await,
        "first\nsecond\n"
    );
    engine.stop().await;
}

/// A pane respawned while the paste settles runs another process: it gets no
/// Enter, and the send reports its outcome unknown: the text is in the pane.
#[tokio::test(flavor = "multi_thread")]
async fn a_respawned_pane_gets_no_enter() {
    let Some(server) = Server::start() else {
        return;
    };
    let file = scratch_file(&server, "before");
    let after = scratch_file(&server, "after");
    cat_session(&server, &file);
    let engine = Engine::start(server.socket());
    let pane = seeded_pane(&engine).await;
    let delivered = engine.deliver(pane, "stale".into(), true).await.unwrap();
    eventually("the paste", || {
        server
            .run(&["capture-pane", "-p", "-t", "agent"])
            .contains("stale")
            .then_some(())
    })
    .await;
    server.run(&[
        "respawn-pane",
        "-k",
        "-t",
        "agent",
        &format!("exec cat -v > {}", after.display()),
    ]);
    let error = delivered.await.expect_err("the send reported success");
    // The text went in, its Enter did not: unknown, not refused (#183).
    assert!(error.is::<NotSubmitted>(), "{error:#}");
    assert_eq!(read_back(&server, &engine, pane, &after).await, "");
    let _ = std::fs::remove_file(&file);
    engine.stop().await;
}

/// A fake harness, written to `dir`: it starts slowly, then reads its
/// prompt raw, writing what it reads, `cat -v`, to the file it is given.
/// `prompt` is what it draws first: with `\033[?2004h` it asks for
/// bracketed paste, as Claude Code and Codex do.
fn fake_harness(dir: &std::path::Path, prompt: &str) -> std::path::PathBuf {
    let path = dir.join("fake-agent");
    write_script(
        &path,
        format!(
            "#!/bin/sh\nsleep 1\nprintf 'fake agent starting\\r\\n'\nsleep 0.5\nstty raw -echo\nprintf '{prompt}'\nexec cat -v > \"$1\"\n"
        ),
    );
    path
}

/// As a launch does: a session whose shell is then typed the harness's
/// command line, and its pane once seeded.
async fn launched(
    server: &Server,
    engine: &Engine,
    harness: &std::path::Path,
    file: &std::path::Path,
) -> u32 {
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "agent",
        "-x",
        "100",
        "-y",
        "30",
        "env INPUTRC=/dev/null PS1='$ ' bash --norc --noprofile",
    ]);
    let pane = seeded_pane(engine).await;
    let line = format!("{} {}", harness.display(), file.display());
    server.run(&["send-keys", "-t", "agent", "-l", &line]);
    server.run(&["send-keys", "-t", "agent", "Enter"]);
    pane
}

/// What the fake harness has read so far.
fn read(file: &std::path::Path) -> String {
    std::fs::read_to_string(file).unwrap_or_default()
}

/// Start through the real launch path with a controlled interactive shell.
async fn launched_shell(server: &Server, engine: &Engine, startup: &str) -> u32 {
    server.new_session("bootstrap", 80, 24);
    let rc = server.dir().join("bashrc");
    std::fs::write(&rc, startup).unwrap();
    let shell = server.dir().join("shell");
    write_script(
        &shell,
        format!(
            "#!/bin/sh\nif [ \"$1\" = -c ]; then exec /bin/bash \"$@\"; fi\nexec /bin/bash --noprofile --rcfile '{}' -i\n",
            rc.display()
        ),
    );
    server.run(&["set-option", "-g", "default-shell", shell.to_str().unwrap()]);
    let launch = crate::launch::Launch {
        harness: "shell".into(),
        folder: server.dir().to_string_lossy().into_owned(),
        name: "fresh".into(),
        flags: String::new(),
        user: String::new(),
        notify: false,
        size: None,
    };
    let result = crate::launch::launch(
        engine,
        &crate::launch::Config::default(),
        &launch,
        Err("none"),
    )
    .await
    .unwrap();
    engine
        .snapshot()
        .sessions
        .iter()
        .find(|session| session.id == result.id)
        .unwrap()
        .panes[0]
        .id
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paste_into_a_fresh_shell_stays_unsubmitted() {
    let Some(server) = Server::start() else {
        return;
    };
    let engine = Engine::start(server.socket());
    let pane = launched_shell(&server, &engine, "PS1='$ '\n").await;
    let marker = server.dir().join("executed");
    engine
        .deliver_when_ready(
            pane,
            format!("touch {}\nsecond line", marker.display()),
            false,
            Duration::from_secs(10),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    eventually("both lines unsubmitted", || {
        let screen = server.run(&["capture-pane", "-p", "-t", "fresh"]);
        (screen.contains("touch ") && screen.contains("second line")).then_some(())
    })
    .await;
    assert!(!marker.exists());
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unbracketed_startup_question_gets_no_enter() {
    let Some(server) = Server::start() else {
        return;
    };
    let engine = Engine::start(server.socket());
    let marker = server.dir().join("answered");
    let pane = launched_shell(
        &server,
        &engine,
        &format!(
            "read -p '[Y/n]' answer\nprintf '%s' \"$answer\" > '{}'\n",
            marker.display()
        ),
    )
    .await;
    eventually("startup question", || {
        server
            .run(&["capture-pane", "-p", "-t", "fresh"])
            .contains("[Y/n]")
            .then_some(())
    })
    .await;
    let error = engine
        .deliver_when_ready(
            pane,
            "hello\ntouch bad".into(),
            true,
            Duration::from_secs(3),
        )
        .await
        .unwrap()
        .await
        .unwrap_err();
    assert!(error.is::<NotReady>());
    assert!(!marker.exists());
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unready_shell_preserves_half_typed_input() {
    let Some(server) = Server::start() else {
        return;
    };
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "agent",
        "env INPUTRC=/dev/null PS1='$ ' bash --norc --noprofile",
    ]);
    eventually("shell prompt", || {
        server
            .run(&["capture-pane", "-p", "-t", "agent"])
            .lines()
            .any(|l| l.trim_end() == "$")
            .then_some(())
    })
    .await;
    let marker = server.dir().join("executed");
    server.run(&[
        "send-keys",
        "-t",
        "agent",
        "-l",
        &format!("touch {}", marker.display()),
    ]);
    let engine = Engine::start(server.socket());
    let pane = seeded_pane(&engine).await;
    let error = engine
        .deliver_when_ready(pane, "hello".into(), false, Duration::from_secs(3))
        .await
        .unwrap()
        .await
        .unwrap_err();
    assert!(error.is::<NotReady>());
    assert!(!marker.exists());
    assert!(
        server
            .run(&["capture-pane", "-p", "-t", "agent"])
            .contains("touch ")
    );
    engine.stop().await;
}

/// An unready shell with a background child must remain unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn unready_shell_with_background_child_is_unchanged() {
    let Some(server) = Server::start() else {
        return;
    };
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "agent",
        "env INPUTRC=/dev/null PS1='$ ' bash --norc --noprofile",
    ]);
    server.run(&["send-keys", "-t", "agent", "sleep 30 &", "Enter"]);
    eventually("the shell prompt after starting a child", || {
        let screen = server.run(&["capture-pane", "-p", "-t", "agent"]);
        (screen.contains("sleep 30") && screen.lines().any(|line| line.trim_end() == "$"))
            .then_some(())
    })
    .await;
    let before = server.run(&["capture-pane", "-p", "-t", "agent"]);
    let engine = Engine::start(server.socket());
    let pane = seeded_pane(&engine).await;
    let error = engine
        .deliver_when_ready(pane, "hello".into(), false, Duration::from_secs(3))
        .await
        .unwrap()
        .await
        .unwrap_err();
    assert!(error.is::<NotReady>());
    assert_eq!(server.run(&["capture-pane", "-p", "-t", "agent"]), before);
    server.run(&["send-keys", "-t", "agent", "kill %1", "Enter"]);
    engine.stop().await;
}

/// #242: a message sent into a just-launched agent waits until the agent
/// has asked for bracketed paste, then arrives whole in its prompt as one
/// paste, unsubmitted. Pasted into the shell before the agent starts, its
/// line break would have run its first line.
#[tokio::test(flavor = "multi_thread")]
async fn a_paste_into_a_fresh_agent_waits_until_it_is_ready() {
    let Some(server) = Server::start() else {
        return;
    };
    let dir = fresh_dir("parvi-242-ready");
    let harness = fake_harness(&dir, "\\033[?2004h> ");
    let file = dir.join("read");
    let engine = Engine::start(server.socket());
    let pane = launched(&server, &engine, &harness, &file).await;
    let started = Instant::now();
    engine
        .deliver_when_ready(
            pane,
            "line one\nline two".into(),
            false,
            Duration::from_secs(10),
        )
        .await
        .unwrap()
        .await
        .expect("delivered once the agent was ready");
    assert!(
        started.elapsed() >= Duration::from_millis(1500),
        "pasted {:?} after the launch, before the agent was up",
        started.elapsed()
    );
    let whole = "^[[200~line one^Mline two^[[201~";
    eventually("the agent to read the paste", || {
        (read(&file) == whole).then_some(())
    })
    .await;
    engine.stop().await;
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #242: a pane whose program never asks for bracketed paste is never
/// ready: it gets nothing, and the sender hears that it was not ready.
#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_pane_that_never_gets_ready_gets_nothing() {
    let Some(server) = Server::start() else {
        return;
    };
    let dir = fresh_dir("parvi-242-never");
    let harness = fake_harness(&dir, "> ");
    let file = dir.join("read");
    let engine = Engine::start(server.socket());
    let pane = launched(&server, &engine, &harness, &file).await;
    let started = Instant::now();
    let error = engine
        .deliver_when_ready(pane, "hello".into(), true, Duration::from_secs(4))
        .await
        .unwrap()
        .await
        .expect_err("a pane that never got ready took the paste");
    assert!(error.is::<NotReady>(), "{error:#}");
    assert!(started.elapsed() >= Duration::from_secs(4));
    // The fake has been reading since well before the deadline.
    assert_eq!(read(&file), "");
    engine.stop().await;
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #242: a fresh agent that opens on a question, such as whether to trust
/// the folder, gets nothing: the paste and its line breaks would answer
/// it. The sender hears at once, not at the deadline.
#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_agent_asking_a_question_gets_nothing() {
    let Some(server) = Server::start() else {
        return;
    };
    let dir = fresh_dir("parvi-242-asks");
    let question = "\\033[?2004h Do you trust the files in this folder?\\r\\n\\r\\n ❯ 1. Yes, proceed\\r\\n   2. No, exit\\r\\n\\r\\n Enter to confirm · Esc to cancel\\r\\n";
    let harness = fake_harness(&dir, question);
    let file = dir.join("read");
    let engine = Engine::start(server.socket());
    let pane = launched(&server, &engine, &harness, &file).await;
    let started = Instant::now();
    let error = engine
        .deliver_when_ready(pane, "yes\nplease".into(), true, Duration::from_secs(20))
        .await
        .unwrap()
        .await
        .expect_err("a question took the paste");
    assert!(error.is::<NotReady>(), "{error:#}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "heard only at the deadline: {:?}",
        started.elapsed()
    );
    assert_eq!(read(&file), "");
    engine.stop().await;
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unrecognised_harness_question_receives_nothing() {
    let Some(server) = Server::start() else {
        return;
    };
    let dir = fresh_dir("parvi-242-unrecognised");
    let harness = fake_harness(&dir, "Continue? [y/N] ");
    let file = dir.join("read");
    let engine = Engine::start(server.socket());
    launched_shell(&server, &engine, "PS1='$ '\n").await;
    let config =
        crate::launch::Config::parse(&format!("harness fake {}", harness.display())).unwrap();
    let request = crate::launch::Launch {
        harness: "fake".into(),
        folder: dir.to_string_lossy().into_owned(),
        name: "agent".into(),
        flags: file.to_string_lossy().into_owned(),
        user: String::new(),
        notify: false,
        size: None,
    };
    let result = crate::launch::launch(&engine, &config, &request, Err("none"))
        .await
        .unwrap();
    assert!(result.warning.is_none(), "{:?}", result.warning);
    let pane = engine
        .snapshot()
        .sessions
        .iter()
        .find(|s| s.id == result.id)
        .unwrap()
        .panes[0]
        .id;
    eventually("question", || {
        server
            .run(&["capture-pane", "-p", "-t", "agent"])
            .contains("[y/N]")
            .then_some(())
    })
    .await;
    let error = engine
        .deliver_when_ready(pane, "yes\nplease".into(), true, Duration::from_secs(3))
        .await
        .unwrap()
        .await
        .unwrap_err();
    assert!(error.is::<NotReady>());
    assert_eq!(read(&file), "");
    engine.stop().await;
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A harness the operator has signed in, and how to get past its first screen.
struct Harness {
    name: &'static str,
    /// On screen once the prompt takes text.
    ready: &'static str,
    trust: Trust,
}

/// The harness's question whether to trust the folder.
struct Trust {
    /// On screen while it asks.
    asks: &'static str,
    /// The tmux key that moves to yes, if yes is not the default.
    to_yes: Option<&'static str>,
    /// On screen once yes is chosen.
    yes: &'static str,
}

const HARNESSES: [Harness; 2] = [
    Harness {
        name: "claude",
        ready: "❯",
        trust: Trust {
            asks: "trust this folder",
            to_yes: Some("Down"),
            yes: "❯ Yes",
        },
    },
    Harness {
        name: "codex",
        ready: "›",
        trust: Trust {
            asks: "Trust this folder",
            to_yes: None,
            yes: "› 1. Trust",
        },
    },
];

/// Polls the pane's screen until `check` passes, for up to `limit`.
async fn screen_until(
    server: &Server,
    limit: Duration,
    what: &str,
    check: impl Fn(&str) -> bool,
) -> String {
    let deadline = Instant::now() + limit;
    loop {
        let screen = server.run(&["capture-pane", "-p", "-t", "seat"]);
        if check(&screen) {
            return screen;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; screen:\n{screen}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Starts the harness in a scratch tmux and leaves it at an empty prompt.
async fn seat(server: &Server, harness: &Harness, folder: &std::path::Path) {
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "seat",
        "-x",
        "120",
        "-y",
        "40",
        "-c",
        &folder.display().to_string(),
        harness.name,
    ]);
    let trust = &harness.trust;
    let screen = screen_until(server, Duration::from_secs(60), "the harness", |s| {
        s.contains(trust.asks) || s.contains(harness.ready)
    })
    .await;
    if screen.contains(trust.asks) {
        // The question is drawn a moment before it takes keys, and Enter on
        // the default answer may quit.
        let deadline = Instant::now() + Duration::from_secs(30);
        while let Some(key) = trust.to_yes
            && !server
                .run(&["capture-pane", "-p", "-t", "seat"])
                .contains(trust.yes)
        {
            assert!(
                Instant::now() < deadline,
                "{} never chose yes",
                harness.name
            );
            server.run(&["send-keys", "-t", "seat", key]);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        screen_until(server, Duration::from_secs(10), "yes", |s| {
            s.contains(trust.yes)
        })
        .await;
        server.run(&["send-keys", "-t", "seat", "Enter"]);
        screen_until(server, Duration::from_secs(60), "the prompt", |s| {
            !s.contains(trust.asks) && s.contains(harness.ready)
        })
        .await;
    }
    // The prompt is drawn a moment before it takes input.
    tokio::time::sleep(Duration::from_secs(2)).await;
}

/// Asks for a sum the text does not contain, so the answer on screen proves
/// the prompt was submitted and read.
fn prompt(round: u32, padding: usize) -> (String, String) {
    let mut text = format!("Do not use any tools. Reply with only the result of 6300 + {round}.");
    for line in 1..=padding {
        text.push_str(&format!(
            "\nPadding line {line}: ignore this line, it only makes the paste large."
        ));
    }
    (text, format!("{}", 6300 + round))
}

/// Issue #63: Send submits to a real agent seat every time, a large
/// bracketed paste included.
#[tokio::test(flavor = "multi_thread")]
async fn agent_seats_submit() {
    let Ok(wanted) = std::env::var("PARVI_AGENT_SEATS") else {
        eprintln!("skipping: set PARVI_AGENT_SEATS=claude,codex to drive real agent seats");
        return;
    };
    for name in wanted.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let harness = HARNESSES
            .iter()
            .find(|h| h.name == name)
            .unwrap_or_else(|| panic!("no harness called {name}"));
        let server = Server::start().expect("tmux is installed");
        let folder = std::env::temp_dir().join(format!("{}-63-seat", server.name));
        std::fs::create_dir_all(&folder).unwrap();
        seat(&server, harness, &folder).await;
        let engine = Engine::start(server.socket());
        let pane = seeded_pane(&engine).await;
        // A large paste, a one-liner, and a few lines.
        for (round, padding) in [(1, 800), (2, 0), (3, 12)] {
            let (text, answer) = prompt(round, padding);
            engine
                .deliver(pane, text, true)
                .await
                .unwrap()
                .await
                .unwrap_or_else(|e| panic!("{name}: round {round}: {e:#}"));
            screen_until(
                &server,
                Duration::from_secs(120),
                &format!("{name} to answer round {round} with {answer}"),
                |s| s.contains(&answer),
            )
            .await;
        }
        engine.stop().await;
        drop(server);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
