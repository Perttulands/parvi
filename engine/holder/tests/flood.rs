//! A session pouring out output, with clients that stop reading it, does not
//! stall the holder: other sessions answer at once, memory stays bounded,
//! and the stuck clients are paused or dropped as tmux would.

#![cfg(windows)]

mod common;

use std::time::{Duration, Instant};

use common::process::{MB, memory};
use common::{Scratch, new_session, run, type_line, wait_for};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};

const FLOODS: [&str; 3] = ["flood", "flood2", "flood3"];

/// Writes 500 lines of 190 characters per write, for ever.
const FLOOD: &str =
    "$l = 'x' * 190; $b = ($l + \"`n\") * 500; while ($true) { [Console]::Out.Write($b) }";

#[tokio::test(flavor = "multi_thread")]
async fn a_flood_does_not_stall_the_holder() {
    let holder = Scratch::start().await;
    let mut c = holder.connect().await;
    let script = holder.dir.join("flood.ps1");
    std::fs::write(&script, FLOOD).unwrap();
    new_session(&mut c, "quiet", "cmd.exe", ">").await;
    // ConPTY passes on well under a megabyte a second per session here, so
    // three floods load the holder more than one.
    let flood = format!("powershell.exe -NoProfile -File {}", script.display());
    for name in FLOODS {
        let mut words = vec!["new-session", "-d", "-s", name, "-x", "200", "-y", "50"];
        words.extend(flood.split(' '));
        run(&mut c, &words).await;
    }
    let (before, _) = memory(holder.pid).unwrap();

    // A client per flood keeps up and counts what it gets; two clients of
    // the first flood stop reading, one with pause-after and one without.
    let mut counters = Vec::new();
    for name in FLOODS {
        let attached = holder
            .connect()
            .await
            .attach(name, "ignore-size")
            .await
            .unwrap();
        counters.push(tokio::spawn(async move {
            let mut reader = attached.reader;
            let mut buf = vec![0u8; 1 << 16];
            let mut total = 0usize;
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until {
                total += reader.read(&mut buf).await.unwrap();
            }
            total
        }));
    }
    let mut paused = holder
        .connect()
        .await
        .attach("flood", "ignore-size,pause-after=1")
        .await
        .unwrap();
    let dropped = holder
        .connect()
        .await
        .attach("flood", "ignore-size")
        .await
        .unwrap();

    // Meanwhile another session answers promptly.
    let mut slowest = Duration::ZERO;
    let until = Instant::now() + Duration::from_secs(5);
    let mut n = 0;
    while Instant::now() < until {
        n += 1;
        let asked = Instant::now();
        let name = run(
            &mut c,
            &["display-message", "-p", "-t", "quiet", "#{session_name}"],
        )
        .await;
        assert_eq!(name, "quiet");
        type_line(&mut c, "quiet", &format!("echo ping-{n}")).await;
        wait_for(&mut c, "quiet", &format!("\nping-{n}")).await;
        slowest = slowest.max(asked.elapsed());
    }
    let mut flowed = 0;
    for counter in counters {
        flowed += counter.await.unwrap();
    }
    let (after, _) = memory(holder.pid).unwrap();
    println!(
        "flood: {:.1} MB in 5 s to the reading clients; slowest answer from the quiet \
         session {slowest:?} over {n} rounds; holder private {:.1} -> {:.1} MB",
        flowed as f64 / MB,
        before as f64 / MB,
        after as f64 / MB
    );
    assert!(flowed > 1 << 20, "the floods barely flowed: {flowed} bytes");
    assert!(
        slowest < Duration::from_secs(2),
        "the quiet session took {slowest:?}"
    );
    // Each flood fills its pane's history (2,000 lines of 200 columns, about
    // 14 MB); client queues are bounded at 1 MB each.
    assert!(
        after < before + FLOODS.len() * (20 << 20) + (16 << 20),
        "holder memory grew by {} bytes",
        after - before
    );

    // The client with pause-after finds its pane paused; the other was
    // dropped.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read =
            tokio::time::timeout_at(deadline.into(), paused.reader.read_until(b'\n', &mut line));
        assert!(read.await.expect("never saw %pause").unwrap() > 0);
        if line.starts_with(b"%pause %") {
            break;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let clients = run(
            &mut c,
            &["list-clients", "-t", "flood", "-F", "#{client_flags}"],
        )
        .await;
        if clients.lines().count() == 1 {
            assert!(clients.contains("pause-after=1"), "{clients}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the client that stopped reading without pause-after is still attached:\n{clients}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drop(dropped);

    run(&mut c, &["kill-session", "-t", "flood"]).await;
    let deadline = Instant::now() + Duration::from_secs(30);
    while c
        .command(&["has-session", "-t", "flood"])
        .await
        .unwrap()
        .is_ok()
    {
        assert!(Instant::now() < deadline, "the flood outlived kill-session");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
