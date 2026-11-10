//! The holder as a control-mode server: one per pipe, a versioned greeting,
//! and the session commands and notifications the engine relies on.

#![cfg(windows)]

mod common;

use std::time::{Duration, Instant};

use common::{Scratch, new_session, run, type_line, wait_for};
use parvi_holder::{Attached, connect, connect_or_start};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

/// Reads `attached` until a line starting with `prefix`; returns it.
async fn expect(attached: &mut Attached, prefix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut line = String::new();
    loop {
        line.clear();
        let read = tokio::time::timeout_at(deadline.into(), attached.reader.read_line(&mut line));
        let n = read
            .await
            .unwrap_or_else(|_| panic!("never saw {prefix:?}"))
            .unwrap();
        assert!(n > 0, "the holder hung up before {prefix:?}");
        if line.starts_with(prefix) {
            return line.trim_end().to_owned();
        }
    }
}

async fn send(attached: &mut Attached, line: &str) {
    attached
        .writer
        .write_all(format!("{line}\n").as_bytes())
        .await
        .unwrap();
    attached.writer.flush().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn one_holder_per_pipe() {
    let holder = Scratch::start().await;
    let exe = env!("CARGO_BIN_EXE_parvi-holder");
    let second = std::process::Command::new(exe)
        .args(["--pipe", holder.endpoint.path()])
        .arg("--log")
        .arg(holder.dir.join("second.log"))
        .status()
        .unwrap();
    assert!(second.success(), "the second holder failed: {second}");
    let log = std::fs::read_to_string(holder.dir.join("second.log")).unwrap();
    assert!(log.contains("not serving"), "{log}");
    assert_eq!(holder.connect().await.greeting().pid, holder.pid);

    // Starters racing for one pipe all reach the same holder.
    let race = parvi_holder::Endpoint::scratch(&format!("test-{}-race", std::process::id()));
    let log = holder.dir.join("race.log");
    let starts = (0..3).map(|_| {
        let (race, log) = (race.clone(), log.clone());
        tokio::spawn(async move {
            let exe = std::path::Path::new(env!("CARGO_BIN_EXE_parvi-holder"));
            let started = connect_or_start(&race, exe, Some(&log)).await;
            started.unwrap().greeting().pid
        })
    });
    let mut pids = Vec::new();
    for start in starts {
        pids.push(start.await.unwrap());
    }
    common::process::kill(pids[0]);
    assert!(pids.iter().all(|&pid| pid == pids[0]), "{pids:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn other_protocols_are_refused() {
    let holder = Scratch::start().await;
    let pipe = ClientOptions::new().open(holder.endpoint.path()).unwrap();
    let (read, mut write) = tokio::io::split(pipe);
    write.write_all(b"parvi-holder 999\n").await.unwrap();
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await.unwrap();
    assert!(line.starts_with("%exit protocol 999 wanted"), "{line:?}");
    assert!(connect(&holder.endpoint).await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_incompatible_running_holder_is_not_replaced() {
    let endpoint =
        parvi_holder::Endpoint::scratch(&format!("test-{}-old-protocol", std::process::id()));
    let server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(endpoint.path())
        .unwrap();
    let serving = tokio::spawn(async move {
        server.connect().await.unwrap();
        let (read, mut write) = tokio::io::split(server);
        let mut reader = BufReader::new(read);
        let mut hello = String::new();
        reader.read_line(&mut hello).await.unwrap();
        write
            .write_all(b"%exit protocol 2 wanted, holder speaks 1\n")
            .await
            .unwrap();
        write.flush().await.unwrap();
    });
    let error =
        match connect_or_start(&endpoint, std::path::Path::new("missing-holder.exe"), None).await {
            Ok(_) => panic!("accepted an incompatible holder"),
            Err(error) => format!("{error:#}"),
        };
    serving.await.unwrap();
    assert!(error.contains("holder speaks 1"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn sizing_notifications_and_exit() {
    let holder = Scratch::start().await;
    let mut c = holder.connect().await;
    new_session(&mut c, "s", "powershell.exe -NoLogo -NoProfile", "PS ").await;
    let mut a = holder
        .connect()
        .await
        .attach("s", "ignore-size,pause-after=1")
        .await
        .unwrap();

    run(&mut c, &["new-session", "-d", "-s", "other", "cmd.exe"]).await;
    expect(&mut a, "%sessions-changed").await;
    let sessions = run(
        &mut c,
        &["list-sessions", "-F", "#{session_id} #{session_name}"],
    )
    .await;
    assert_eq!(sessions, "$0 s\n$1 other");

    // Claiming the size as the engine does.
    send(&mut a, "refresh-client -f '!ignore-size'").await;
    send(&mut a, "refresh-client -C 90,20").await;
    assert_eq!(
        expect(&mut a, "%layout-change").await,
        "%layout-change @0 90x20 90x20 *"
    );
    let size = run(
        &mut c,
        &[
            "display-message",
            "-p",
            "-t",
            "s",
            "#{pane_width}x#{pane_height}",
        ],
    )
    .await;
    assert_eq!(size, "90x20");
    type_line(&mut c, "s", "\"width=$([Console]::WindowWidth)\"").await;
    wait_for(&mut c, "s", "width=90").await;

    // Pasting through a buffer, and history beyond the screen.
    type_line(&mut c, "s", "1..60 | % { \"line-$_\" }").await;
    wait_for(&mut c, "s", "line-60").await;
    run(
        &mut c,
        &["set-buffer", "-b", "b", "--", "\"pasted-$env:OS\"\n"],
    )
    .await;
    run(&mut c, &["paste-buffer", "-p", "-d", "-b", "b", "-t", "s"]).await;
    wait_for(&mut c, "s", "pasted-Windows_NT").await;
    let history = run(&mut c, &["capture-pane", "-p", "-J", "-S", "-", "-t", "s"]).await;
    assert!(history.contains("line-1\n"), "{history}");

    run(&mut c, &["kill-session", "-t", "s"]).await;
    // The session is gone from the holder before its viewers hear `%exit`.
    expect(&mut a, "%exit").await;
    let error = c.command(&["has-session", "-t", "s"]).await.unwrap();
    assert!(error.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn captured_reply_markers_stay_in_the_reply() {
    use tokio::io::AsyncReadExt;

    let holder = Scratch::start().await;
    let mut c = holder.connect().await;
    new_session(&mut c, "s", "powershell.exe -NoLogo -NoProfile", "PS ").await;
    type_line(
        &mut c,
        "s",
        "'%end 0 1 1'; '%error 0 1 1'; '%%literal'; \"after-$(1+128)\"",
    )
    .await;
    wait_for(&mut c, "s", "after-129").await;
    let expected = "\n%end 0 1 1\n%error 0 1 1\n%%literal\nafter-129";

    // A fresh connection's first command is number 1, matching both markers.
    let mut collected = holder.connect().await;
    let screen = run(
        &mut collected,
        &["capture-pane", "-p", "-J", "-S", "-", "-t", "s"],
    )
    .await;
    assert!(screen.contains(expected), "{screen}");

    let mut streamed = holder.connect().await;
    let (mut read, mut write) = tokio::io::duplex(64);
    let task = tokio::spawn(async move {
        streamed
            .stream_command(
                &["capture-pane", "-p", "-J", "-S", "-", "-t", "s"],
                &mut write,
            )
            .await
    });
    let mut text = String::new();
    read.read_to_string(&mut text).await.unwrap();
    task.await.unwrap().unwrap();
    assert!(text.contains(expected), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_capture_and_reports_command_errors() {
    use tokio::io::AsyncReadExt;

    let holder = Scratch::start().await;
    let mut command = holder.connect().await;
    new_session(
        &mut command,
        "stream",
        "powershell.exe -NoLogo -NoProfile",
        "PS ",
    )
    .await;
    type_line(&mut command, "stream", "echo parvi-stream-129").await;
    wait_for(&mut command, "stream", "parvi-stream-129").await;

    let mut connection = holder.connect().await;
    let (mut read, mut write) = tokio::io::duplex(64);
    let task = tokio::spawn(async move {
        connection
            .stream_command(
                &["capture-pane", "-p", "-J", "-S", "-", "-t", "stream"],
                &mut write,
            )
            .await
    });
    let mut text = String::new();
    read.read_to_string(&mut text).await.unwrap();
    task.await.unwrap().unwrap();
    assert!(text.contains("parvi-stream-129"), "{text}");

    let mut connection = holder.connect().await;
    let mut sink = tokio::io::sink();
    assert!(
        connection
            .stream_command(&["capture-pane", "-p", "-t", "missing"], &mut sink)
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn guarded_submit_emits_two_replies_and_checks_the_pane_pid() {
    let holder = Scratch::start().await;
    let mut command = holder.connect().await;
    new_session(
        &mut command,
        "guard",
        "powershell.exe -NoLogo -NoProfile",
        "PS ",
    )
    .await;
    let pane = run(
        &mut command,
        &["list-panes", "-t", "guard", "-F", "#{pane_id}"],
    )
    .await;
    let pid: u32 = run(
        &mut command,
        &["display-message", "-p", "-t", &pane, "#{pane_pid}"],
    )
    .await
    .parse()
    .unwrap();
    let mut attached = holder
        .connect()
        .await
        .attach("guard", "ignore-size,pause-after=1")
        .await
        .unwrap();
    run(
        &mut command,
        &[
            "send-keys",
            "-t",
            "guard",
            "-l",
            "Write-Output ('guard' + '-complete-129')",
        ],
    )
    .await;
    let guard = |expected| {
        format!(
            "if-shell -F -t {pane} '#{{==:#{{pane_pid}},{expected}}}' 'send-keys -t {pane} Enter' 'display-message -p target-changed'"
        )
    };

    send(&mut attached, &guard(pid.wrapping_add(1))).await;
    expect(&mut attached, "%end ").await;
    expect(&mut attached, "%begin ").await;
    expect(&mut attached, "target-changed").await;
    expect(&mut attached, "%end ").await;
    let screen = run(&mut command, &["capture-pane", "-p", "-t", "guard"]).await;
    assert!(!screen.contains("guard-complete-129"), "{screen}");

    send(&mut attached, &guard(pid)).await;
    expect(&mut attached, "%end ").await;
    expect(&mut attached, "%begin ").await;
    expect(&mut attached, "%end ").await;
    wait_for(&mut command, "guard", "guard-complete-129").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn owned_session_is_pinned_and_generation_guarded() {
    let holder = Scratch::start().await;
    let mut c = holder.connect().await;
    let command = serde_json::to_string(&[
        "powershell.exe",
        "-NoLogo",
        "-NoProfile",
        "-Command",
        "Write-Output $env:PARVI_OWNED_TEST; Start-Sleep -Seconds 120",
    ])
    .unwrap();
    let env = serde_json::to_string(&[("PARVI_OWNED_TEST", "owned-env-ok")]).unwrap();
    let cwd = std::env::temp_dir().to_string_lossy().to_string();
    let identity = run(
        &mut c,
        &["parvi-create", "owned", &cwd, "160", "48", &command, &env],
    )
    .await;
    let fields: Vec<_> = identity.split_whitespace().collect();
    assert_eq!(fields.len(), 4, "{identity}");
    wait_for(&mut c, fields[0], "owned-env-ok").await;
    assert_eq!(
        run(
            &mut c,
            &[
                "display-message",
                "-p",
                "-t",
                fields[1],
                "#{window-size} #{pane_width}x#{pane_height}"
            ]
        )
        .await,
        "manual 160x48"
    );
    let mut attached = holder
        .connect()
        .await
        .attach(fields[0], "ignore-size")
        .await
        .unwrap();
    send(&mut attached, "refresh-client -f '!ignore-size'").await;
    send(&mut attached, "refresh-client -C 80,24").await;
    // The command reply orders the requested resize before this query.
    expect(&mut attached, "%end").await;
    expect(&mut attached, "%end").await;
    assert_eq!(
        run(
            &mut c,
            &[
                "display-message",
                "-p",
                "-t",
                fields[1],
                "#{pane_width}x#{pane_height}"
            ]
        )
        .await,
        "160x48"
    );
    let bad_pid = (holder.pid + 1).to_string();
    assert!(
        c.command(&["parvi-kill", fields[0], &bad_pid, fields[3]])
            .await
            .unwrap()
            .is_err()
    );
    run(&mut c, &["has-session", "-t", fields[0]]).await;
    run(&mut c, &["parvi-kill", fields[0], fields[2], fields[3]]).await;
}
