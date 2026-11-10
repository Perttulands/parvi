//! What an idle session costs, measured: the holder's own share, the
//! console host every ConPTY needs, and the shells.

#![cfg(windows)]

mod common;

use std::time::Duration;

use common::process::{MB, children, memory, snapshot};
use common::{Scratch, new_session};

const CMD_SESSIONS: usize = 10;

#[tokio::test(flavor = "multi_thread")]
async fn memory_per_idle_session() {
    let holder = Scratch::start().await;
    let mut c = holder.connect().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (empty, _) = memory(holder.pid).unwrap();

    for n in 0..CMD_SESSIONS {
        new_session(&mut c, &format!("cmd{n}"), "cmd.exe", ">").await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (with_cmd, with_cmd_ws) = memory(holder.pid).unwrap();
    let per_session = (with_cmd - empty) as f64 / CMD_SESSIONS as f64;

    new_session(&mut c, "ps5", "powershell.exe -NoLogo -NoProfile", "PS ").await;
    new_session(&mut c, "ps7", "pwsh.exe -NoLogo -NoProfile", "PS ").await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    println!("idle session memory, private / working set, MB:");
    println!(
        "  holder, empty: {:.2}; with {CMD_SESSIONS} cmd sessions: {:.2} / {:.2}; per session: {:.2}",
        empty as f64 / MB,
        with_cmd as f64 / MB,
        with_cmd_ws as f64 / MB,
        per_session / MB
    );
    let all = snapshot();
    let mut by_exe: std::collections::BTreeMap<String, Vec<(usize, usize)>> = Default::default();
    for (pid, exe) in children(&all, holder.pid) {
        if let Some(used) = memory(pid) {
            by_exe.entry(exe).or_default().push(used);
        }
    }
    for (exe, used) in &by_exe {
        let count = used.len() as f64;
        let private = used.iter().map(|u| u.0).sum::<usize>() as f64 / count / MB;
        let ws = used.iter().map(|u| u.1).sum::<usize>() as f64 / count / MB;
        println!("  {exe} x{}: {private:.2} / {ws:.2} each", used.len());
    }
    assert!(
        per_session < 4.0 * MB,
        "the holder spends {:.2} MB per idle session",
        per_session / MB
    );
}
