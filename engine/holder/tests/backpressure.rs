//! Clients that push more than a program or a client takes are refused or
//! dropped, and the holder's memory stays bounded.

#![cfg(windows)]

mod common;

use std::time::{Duration, Instant};

use common::process::{MB, memory};
use common::{Scratch, new_session, run, type_line, wait_for};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread")]
async fn input_to_a_program_that_is_not_reading_is_refused() {
    let holder = Scratch::start().await;
    let mut c = holder.connect().await;
    run(
        &mut c,
        &[
            "new-session",
            "-d",
            "-s",
            "asleep",
            "powershell.exe",
            "-NoProfile",
            "-Command",
            "Start-Sleep",
            "600",
        ],
    )
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (before, _) = memory(holder.pid).unwrap();
    let paste = "x".repeat(64 * 1024);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut sent = 0usize;
    let refusal = loop {
        let typed = c
            .command(&["send-keys", "-t", "asleep", "-l", &paste])
            .await;
        match typed.unwrap() {
            Ok(_) => sent += paste.len(),
            Err(message) => break message,
        }
        assert!(
            Instant::now() < deadline,
            "{sent} bytes of input all accepted"
        );
    };
    let (after, _) = memory(holder.pid).unwrap();
    println!(
        "input: refused after {:.1} MB: {refusal}; holder private {:.1} -> {:.1} MB",
        sent as f64 / MB,
        before as f64 / MB,
        after as f64 / MB
    );
    assert!(refusal.contains("input refused"), "{refusal}");
    assert!(
        after < before + (16 << 20),
        "holder memory grew by {} bytes",
        after - before
    );
    // The refusal costs nothing else: other commands still answer.
    run(&mut c, &["has-session", "-t", "asleep"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn replies_a_client_does_not_read_are_bounded() {
    let holder = Scratch::start().await;
    let mut c = holder.connect().await;
    new_session(&mut c, "s", "powershell.exe -NoLogo -NoProfile", "PS ").await;
    type_line(
        &mut c,
        "s",
        "1..2000 | % { \"$_ \" + ('y' * 100) }; \"done-$(1+1)\"",
    )
    .await;
    wait_for(&mut c, "s", "done-2").await;
    let (before, _) = memory(holder.pid).unwrap();

    // A client pipelines whole-history captures and reads nothing.
    let pipe = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(holder.endpoint.path())
        .unwrap();
    let (mut read, mut write) = tokio::io::split(pipe);
    let mut lines = parvi_holder::handshake::hello();
    for _ in 0..300 {
        lines.push_str("capture-pane -p -e -S - -t s\n");
    }
    write.write_all(lines.as_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let (after, _) = memory(holder.pid).unwrap();
    println!(
        "replies: holder private {:.1} -> {:.1} MB",
        before as f64 / MB,
        after as f64 / MB
    );
    assert!(
        after < before + (32 << 20),
        "holder memory grew by {} bytes",
        after - before
    );

    // It was dropped: reading finds the end of the connection.
    let mut sink = vec![0u8; 1 << 16];
    let ended = tokio::time::timeout(Duration::from_secs(30), async {
        while read.read(&mut sink).await.unwrap_or(0) > 0 {}
    });
    ended
        .await
        .expect("the client that stopped reading was never dropped");
    run(&mut c, &["has-session", "-t", "s"]).await;
}
