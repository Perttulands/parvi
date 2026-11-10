//! A scratch holder per test, and what the tests look at from outside it.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use parvi_holder::{Connection, Endpoint, connect, connect_or_start};

pub mod process;

/// A holder of the test's own on a scratch pipe. Dropping it ends the holder
/// and its sessions.
pub struct Scratch {
    pub endpoint: Endpoint,
    pub pid: u32,
    pub dir: PathBuf,
}

static NEXT: AtomicU32 = AtomicU32::new(0);

impl Scratch {
    pub async fn start() -> Scratch {
        let name = format!(
            "test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join("parvi-holder-tests").join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        let endpoint = Endpoint::scratch(&name);
        let exe = PathBuf::from(env!("CARGO_BIN_EXE_parvi-holder"));
        let connection = connect_or_start(&endpoint, &exe, Some(&dir.join("holder.log")))
            .await
            .expect("starting a scratch holder");
        let pid = connection.greeting().pid;
        Scratch { endpoint, pid, dir }
    }

    pub async fn connect(&self) -> Connection {
        connect(&self.endpoint).await.expect("connecting")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        process::kill(self.pid);
    }
}

/// Runs a command that must succeed; returns its output.
pub async fn run(connection: &mut Connection, words: &[&str]) -> String {
    match connection
        .command(words)
        .await
        .expect("talking to the holder")
    {
        Ok(output) => output,
        Err(message) => panic!("{words:?}: {message}"),
    }
}

/// Types `line` into `session` and presses Enter.
pub async fn type_line(connection: &mut Connection, session: &str, line: &str) {
    run(connection, &["send-keys", "-t", session, "-l", line]).await;
    run(connection, &["send-keys", "-t", session, "Enter"]).await;
}

/// Waits until `session`'s screen shows `needle`; returns the screen.
pub async fn wait_for(connection: &mut Connection, session: &str, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let screen = run(connection, &["capture-pane", "-p", "-t", session]).await;
        if screen.contains(needle) {
            return screen;
        }
        assert!(
            Instant::now() < deadline,
            "{session} never showed {needle:?}; it shows:\n{screen}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Starts a session running `command` and waits for `ready` on its screen.
pub async fn new_session(connection: &mut Connection, name: &str, command: &str, ready: &str) {
    let mut words = vec!["new-session", "-d", "-s", name, "-x", "120", "-y", "30"];
    words.extend(command.split(' '));
    run(connection, &words).await;
    wait_for(connection, name, ready).await;
}
