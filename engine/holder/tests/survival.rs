//! Sessions outlive the client attached to them: kill the client process and
//! the shell, its state and its output carry on.

#![cfg(windows)]

mod common;

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use common::{Scratch, new_session, run, type_line, wait_for};
use parvi_holder::{Endpoint, connect};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const PIPE: &str = "PARVI_HOLDER_TEST_PIPE";
const SESSION: &str = "PARVI_HOLDER_TEST_SESSION";

#[tokio::test(flavor = "multi_thread")]
async fn powershell_5_survives_killing_the_attached_client() {
    survives("powershell.exe -NoLogo -NoProfile", 5).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn powershell_7_survives_killing_the_attached_client() {
    survives("pwsh.exe -NoLogo -NoProfile", 7).await;
}

async fn survives(shell: &str, major: u32) {
    let holder = Scratch::start().await;
    let mut c = holder.connect().await;
    new_session(&mut c, "s", shell, "PS ").await;
    type_line(
        &mut c,
        "s",
        "$x = 42; \"major=$($PSVersionTable.PSVersion.Major)\"",
    )
    .await;
    wait_for(&mut c, "s", &format!("major={major}")).await;

    let mut client = attach_elsewhere(&holder, "s");
    wait_until(&mut c, "the client attached", |clients| !clients.is_empty()).await;
    type_line(&mut c, "s", "echo \"before $x\"").await;
    wait_for(&mut c, "s", "before 42").await;

    client.kill().unwrap();
    client.wait().unwrap();
    wait_until(&mut c, "the client left", |clients| clients.is_empty()).await;

    run(&mut c, &["has-session", "-t", "s"]).await;
    type_line(&mut c, "s", "echo \"after $x\"").await;
    wait_for(&mut c, "s", "after 42").await;

    // A new client attaches and is fed as the old one was.
    let mut attached = holder
        .connect()
        .await
        .attach("s", "ignore-size,pause-after=1")
        .await
        .unwrap();
    attached
        .writer
        .write_all(b"send-keys -t s -l \"echo \\\"again $x\\\"\"\nsend-keys -t s Enter\n")
        .await
        .unwrap();
    attached.writer.flush().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut line = String::new();
    loop {
        line.clear();
        let read = tokio::time::timeout_at(deadline.into(), attached.reader.read_line(&mut line));
        assert!(read.await.expect("no output after reattaching").unwrap() > 0);
        if line.starts_with("%output") && line.contains("again 42") {
            break;
        }
    }
}

/// Waits until the clients attached to `s` pass `done`.
async fn wait_until(c: &mut parvi_holder::Connection, what: &str, done: impl Fn(&str) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(&run(c, &["list-clients", "-t", "s"]).await) {
        assert!(Instant::now() < deadline, "{what}: timed out");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Starts another process that attaches to `session` and reads its output
/// for as long as it lives, as a node does.
fn attach_elsewhere(holder: &Scratch, session: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "attached_client", "--ignored", "--nocapture"])
        .env(PIPE, holder.endpoint.path())
        .env(SESSION, session)
        .spawn()
        .unwrap()
}

/// The client process [`attach_elsewhere`] starts. Does nothing when run
/// any other way.
#[test]
#[ignore = "run by the survival tests as their attached client"]
fn attached_client() {
    let (Ok(pipe), Ok(session)) = (std::env::var(PIPE), std::env::var(SESSION)) else {
        return;
    };
    let name = pipe.trim_start_matches(r"\\.\pipe\parvi-holder-");
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let connection = connect(&Endpoint::scratch(name)).await.unwrap();
        let mut attached = connection
            .attach(&session, "ignore-size,pause-after=1")
            .await
            .unwrap();
        let mut line = String::new();
        while attached.reader.read_line(&mut line).await.unwrap() > 0 {
            line.clear();
        }
    });
}
