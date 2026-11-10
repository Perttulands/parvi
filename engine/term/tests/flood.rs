//! Issue #24: a flood of output never stalls tmux or parvi, memory stays
//! bounded, and the held screen matches tmux once the flood is over. A test
//! binary of its own, so the memory reading is this test's alone.
#![cfg(unix)]

use std::time::{Duration, Instant};

use parvi_term::engine::Engine;
use parvi_testkit::tmux::*;

/// How much more memory the flood may cost us, at its peak.
const MAX_GROWTH_KIB: u64 = 64 * 1024;

/// How long tmux or parvi may take to answer during the flood.
const MAX_STALL: Duration = Duration::from_secs(2);

/// This process's resident memory, where /proc has it.
fn rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a stress test: it floods a pane and loads the machine. Run it on request with `cargo test --test flood -- --ignored`"]
async fn a_flood_never_stalls_tmux_or_us() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("flood", 120, 40);
    server.new_session("quiet", 80, 24);
    let engine = Engine::start(server.socket());
    eventually("both sessions seeded", || {
        all_synced(&engine.snapshot(), &["flood", "quiet"]).then_some(())
    })
    .await;
    let pane_of = |name: &str| {
        let snapshot = engine.snapshot();
        snapshot
            .sessions
            .iter()
            .find(|s| s.name == name)
            .unwrap()
            .panes[0]
            .id
    };
    let (flood, quiet) = (pane_of("flood"), pane_of("quiet"));

    // A viewer that never reads, like a browser tab on a stalled network.
    let _stalled = engine.subscribe(flood).unwrap();
    let before = rss_kib();
    let mut peak = before;

    // Text, then bytes that aren't UTF-8 (control characters removed, so
    // nothing in the flood talks back to the shell), then a known screen.
    // The bytes may end mid-character, right before `clear`'s escapes
    // (issue #29).
    server.run(&[
        "send-keys",
        "-t",
        "flood",
        "head -c 30000000 /dev/urandom | base64 -w 118; \
         head -c 10000000 /dev/urandom | tr -d '\\000-\\037\\177' | fold -b -w 100; \
         clear; seq 1 30; echo flood-$((1+1))-done",
        "Enter",
    ]);

    let started = Instant::now();
    let deadline = started + Duration::from_secs(120);
    let mut typed = 0;
    loop {
        assert!(Instant::now() < deadline, "the flood never finished");

        let asked = Instant::now();
        let screen = capture(&server, "flood");
        let tmux_took = asked.elapsed();
        assert!(tmux_took < MAX_STALL, "tmux took {tmux_took:?} to answer");

        let asked = Instant::now();
        let _ = engine.snapshot();
        let we_took = asked.elapsed();
        assert!(we_took < MAX_STALL, "parvi took {we_took:?} to answer");

        // Typing into another session still gets through, and comes back.
        typed += 1;
        let word = format!("typed-{typed}");
        engine
            .send_input(quiet, format!("echo typed-$(({typed}))\r").into_bytes())
            .await
            .unwrap();
        let asked = Instant::now();
        loop {
            if pane_text(&engine, "quiet").is_some_and(|text| text.contains(&word)) {
                break;
            }
            assert!(
                asked.elapsed() < MAX_STALL,
                "typing into quiet took over {MAX_STALL:?} to come back"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        if let (Some(now), Some(high)) = (rss_kib(), peak) {
            peak = Some(high.max(now));
        }
        if screen.contains("flood-2-done") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    eprintln!(
        "flood took {:?}; memory before {before:?} KiB, peak {peak:?} KiB",
        started.elapsed()
    );
    if let (Some(before), Some(peak)) = (before, peak) {
        assert!(
            peak - before < MAX_GROWTH_KIB,
            "memory grew by {} KiB during the flood",
            peak - before
        );
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ours = pane_text(&engine, "flood").map(|text| normalise(&text));
        let theirs = normalise(&capture(&server, "flood"));
        if ours.as_deref() == Some(theirs.as_str()) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the held screen never matched tmux\nours: {ours:?}\ntmux: {theirs:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    engine.stop().await;
}
