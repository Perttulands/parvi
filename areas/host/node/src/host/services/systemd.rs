//! What a component is doing, and starting or stopping it, through
//! systemd directly: `systemctl --user` for the user's own units (through
//! `sudo -n -u` for another user's), `systemctl show` for a system unit,
//! and a loopback request for an HTTP probe. As CHROTE's controller does.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::catalog::Component;

/// The most a systemctl call may take.
pub const SYSTEMCTL: Duration = Duration::from_secs(15);
/// The most an HTTP probe may take.
const PROBE: Duration = Duration::from_secs(2);

/// `systemctl` for this component and verb (`show`, `start`, `stop`).
fn command(component: &Component, verb: &str) -> Result<Command, String> {
    let props = ["--property=LoadState", "--property=ActiveState"];
    match component {
        Component::System { unit } => {
            let mut c = Command::new("systemctl");
            c.arg("show").args(props).arg(unit);
            Ok(c)
        }
        Component::User { user, unit } => {
            let uid = uid_of(user).ok_or_else(|| format!("no user {user}"))?;
            let env = [
                format!("XDG_RUNTIME_DIR=/run/user/{uid}"),
                format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"),
            ];
            let mut c = if uid == own_uid() {
                Command::new("env")
            } else {
                let mut c = Command::new("sudo");
                c.args(["-n", "-u", user, "env"]);
                c
            };
            c.args(env).args(["systemctl", "--user", verb]);
            if verb == "show" {
                c.args(props);
            }
            c.arg(unit);
            Ok(c)
        }
        Component::Http { .. } => Err("an HTTP probe is not a unit".into()),
    }
    .map(|mut c| {
        // A systemctl that outlives its deadline is killed with the wait
        // for it, so hung calls never pile up behind the node.
        c.kill_on_drop(true);
        c
    })
}

/// `active`, `inactive`, `failed`, `activating`, `deactivating`,
/// `reloading`, or `unknown` when systemd does not say.
pub async fn state(component: &Component) -> &'static str {
    if let Component::Http { url } = component {
        return probe(url).await;
    }
    let Ok(mut c) = command(component, "show") else {
        return "unknown";
    };
    let out = tokio::time::timeout(SYSTEMCTL, c.stdin(Stdio::null()).output()).await;
    match out {
        Ok(Ok(o)) if o.status.success() => active_state(&String::from_utf8_lossy(&o.stdout)),
        _ => "unknown",
    }
}

/// Reads `systemctl show`'s answer.
pub fn active_state(show: &str) -> &'static str {
    let field = |name: &str| {
        show.lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix('='))
            .unwrap_or("")
    };
    if field("LoadState") != "loaded" {
        return "unknown";
    }
    match field("ActiveState") {
        "active" => "active",
        "inactive" => "inactive",
        "failed" => "failed",
        "activating" => "activating",
        "deactivating" => "deactivating",
        "reloading" => "reloading",
        _ => "unknown",
    }
}

/// Why a unit did not do as told.
#[derive(Debug, PartialEq)]
pub enum Refusal {
    /// systemd said no, in its own words.
    Failed(String),
    /// No answer within the deadline. systemctl is gone, but the job it
    /// handed systemd may still finish: only a later read can tell.
    Unconfirmed(String),
}

/// Starts or stops one unit within `within`. The failure is systemctl's
/// own words.
pub async fn act(component: &Component, verb: &str, within: Duration) -> Result<(), Refusal> {
    let mut c = command(component, verb).map_err(Refusal::Failed)?;
    let out = tokio::time::timeout(within, c.stdin(Stdio::null()).output())
        .await
        .map_err(|_| {
            Refusal::Unconfirmed(format!(
                "systemctl {verb} gave no answer in {} s; systemd may still finish it",
                within.as_secs()
            ))
        })?
        .map_err(|e| Refusal::Failed(e.to_string()))?;
    if out.status.success() {
        return Ok(());
    }
    let words = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(Refusal::Failed(if words.is_empty() {
        "service command failed".into()
    } else {
        words.chars().take(500).collect()
    }))
}

/// Any answer at all is up, an error page included; no answer is down.
async fn probe(url: &str) -> &'static str {
    let Some(rest) = url.strip_prefix("http://") else {
        return "inactive";
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let ask = async {
        let mut s = tokio::net::TcpStream::connect(host).await.ok()?;
        let req = format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.ok()?;
        let mut head = [0u8; 5];
        s.read_exact(&mut head).await.ok()?;
        (&head == b"HTTP/").then_some(())
    };
    match tokio::time::timeout(PROBE, ask).await {
        Ok(Some(())) => "active",
        _ => "inactive",
    }
}

#[cfg(unix)]
fn own_uid() -> u32 {
    // SAFETY: getuid cannot fail and touches no memory.
    unsafe { libc::getuid() }
}

#[cfg(unix)]
fn uid_of(user: &str) -> Option<u32> {
    let name = std::ffi::CString::new(user).ok()?;
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is to memory owned here, sized as given.
    let rc = unsafe {
        libc::getpwnam_r(
            name.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut found,
        )
    };
    (rc == 0 && !found.is_null()).then_some(pwd.pw_uid)
}

#[cfg(not(unix))]
fn own_uid() -> u32 {
    0
}

#[cfg(not(unix))]
fn uid_of(_: &str) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_systemds_words() {
        assert_eq!(
            active_state("LoadState=loaded\nActiveState=active\n"),
            "active"
        );
        assert_eq!(
            active_state("ActiveState=failed\nLoadState=loaded"),
            "failed"
        );
        assert_eq!(
            active_state("LoadState=not-found\nActiveState=inactive"),
            "unknown",
            "a unit systemd does not know is unknown, not stopped"
        );
        assert_eq!(
            active_state("LoadState=loaded\nActiveState=maintenance"),
            "unknown"
        );
    }
}
