//! What a component is doing, starting or stopping it and reading its log,
//! through systemd directly: `systemctl --user` and `journalctl --user` for
//! the user's own units (through `sudo -n -u` for another user's),
//! `systemctl show` and `journalctl` for a system unit, and a loopback
//! request for an HTTP probe. As CHROTE's controller does.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use parvi_host_contract::{Component as State, LogLine};

use super::catalog::Component;

/// The most a systemctl call may take.
pub const SYSTEMCTL: Duration = Duration::from_secs(15);
/// The most an HTTP probe may take.
const PROBE: Duration = Duration::from_secs(2);

/// The most log lines one read gives back.
pub const LOG_MOST: usize = 200;

/// What `systemctl show` is asked for.
const PROPS: &str = "--property=LoadState,ActiveState,SubState,NRestarts,\
                     ActiveEnterTimestampMonotonic,InactiveEnterTimestampMonotonic";

/// `program` (`systemctl` or `journalctl`) for this component, as its
/// user's manager or the system's, with `args`.
fn command(component: &Component, program: &str, args: &[&str]) -> Result<Command, String> {
    match component {
        Component::System { unit } => {
            let mut c = Command::new(program);
            c.args(args);
            if program == "journalctl" {
                c.arg("-u");
            }
            c.arg(unit);
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
            c.args(env).args([program, "--user"]).args(args);
            if program == "journalctl" {
                c.arg("-u");
            }
            c.arg(unit);
            Ok(c)
        }
        Component::Http { .. } => Err("an HTTP probe is not a unit".into()),
    }
    .map(|mut c| {
        // A call that outlives its deadline is killed with the wait for
        // it, so hung calls never pile up behind the node.
        c.kill_on_drop(true);
        c
    })
}

/// What the component is doing, as systemd says.
pub async fn state(component: &Component) -> State {
    let unit = match component {
        Component::Http { url } => {
            return State {
                unit: url.clone(),
                state: probe(url).await.into(),
                ..State::default()
            };
        }
        Component::User { unit, .. } | Component::System { unit } => unit.clone(),
    };
    let unknown = State {
        unit: unit.clone(),
        state: "unknown".into(),
        ..State::default()
    };
    let Ok(mut c) = command(component, "systemctl", &["show", PROPS]) else {
        return unknown;
    };
    let out = tokio::time::timeout(SYSTEMCTL, c.stdin(Stdio::null()).output()).await;
    match out {
        Ok(Ok(o)) if o.status.success() => State {
            unit,
            ..shown(
                &String::from_utf8_lossy(&o.stdout),
                now_ms(),
                monotonic_us(),
            )
        },
        _ => unknown,
    }
}

/// Reads `systemctl show`'s answer, at `now` (Unix ms) and `clock` (the
/// monotonic clock, µs, which systemd's `…Monotonic` times count in).
pub fn shown(show: &str, now: u64, clock: u64) -> State {
    let field = |name: &str| {
        show.lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix('='))
            .unwrap_or("")
    };
    if field("LoadState") != "loaded" {
        return State {
            state: "unknown".into(),
            ..State::default()
        };
    }
    let sub = field("SubState").to_owned();
    let state = match (field("ActiveState"), sub.as_str()) {
        // Dead and waiting to be started again: a crash loop, not a start.
        ("activating", "auto-restart") => "failed",
        ("active", _) => "active",
        ("inactive", _) => "inactive",
        ("failed", _) => "failed",
        ("activating", _) => "activating",
        ("deactivating", _) => "deactivating",
        ("reloading", _) => "reloading",
        _ => "unknown",
    };
    let entered = if matches!(state, "active" | "activating" | "reloading") {
        "ActiveEnterTimestampMonotonic"
    } else {
        "InactiveEnterTimestampMonotonic"
    };
    let since = field(entered)
        .parse::<u64>()
        .ok()
        .filter(|us| *us > 0 && *us <= clock)
        .map(|us| now.saturating_sub((clock - us) / 1000));
    State {
        unit: String::new(),
        state: state.into(),
        sub,
        since,
        restarts: field("NRestarts").parse().unwrap_or(0),
    }
}

/// The last `lines` lines `component` logged, oldest first.
pub async fn log(component: &Component, lines: usize) -> Result<Vec<LogLine>, String> {
    let n = lines.min(LOG_MOST).to_string();
    let args = ["-n", n.as_str(), "-o", "json", "--no-pager", "-q"];
    let mut c = command(component, "journalctl", &args)?;
    let out = tokio::time::timeout(SYSTEMCTL, c.stdin(Stdio::null()).output())
        .await
        .map_err(|_| format!("journalctl gave no answer in {} s", SYSTEMCTL.as_secs()))?
        .map_err(|e| format!("journalctl: {e}"))?;
    if !out.status.success() {
        let words = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if words.is_empty() {
            format!("journalctl {}", out.status)
        } else {
            words.chars().take(500).collect()
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(journal_line)
        .collect())
}

/// One `journalctl -o json` entry: its time, message and priority. A
/// message journald keeps as bytes is read as text.
pub fn journal_line(json: &str) -> Option<LogLine> {
    let entry: serde_json::Value = serde_json::from_str(json).ok()?;
    let at = entry["__REALTIME_TIMESTAMP"]
        .as_str()?
        .parse::<u64>()
        .ok()?
        / 1000;
    let text = match &entry["MESSAGE"] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(bytes) => {
            let bytes: Vec<u8> = bytes
                .iter()
                .filter_map(|b| b.as_u64())
                .map(|b| b as u8)
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => return None,
    };
    let priority = entry["PRIORITY"]
        .as_str()
        .and_then(|p| p.parse::<u8>().ok());
    Some(LogLine {
        at,
        text: text.trim_end().to_owned(),
        error: priority.is_some_and(|p| p <= 3),
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The monotonic clock systemd's `…Monotonic` times count, in µs.
#[cfg(unix)]
fn monotonic_us() -> u64 {
    let mut t: libc::timespec = unsafe { std::mem::zeroed() };
    // SAFETY: `t` is valid for writing.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) } != 0 {
        return 0;
    }
    t.tv_sec as u64 * 1_000_000 + t.tv_nsec as u64 / 1000
}

#[cfg(not(unix))]
fn monotonic_us() -> u64 {
    0
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
    let mut c = command(component, "systemctl", &[verb]).map_err(Refusal::Failed)?;
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

    fn state(show: &str) -> String {
        shown(show, 0, 0).state
    }

    #[test]
    fn reads_systemds_words() {
        assert_eq!(state("LoadState=loaded\nActiveState=active\n"), "active");
        assert_eq!(state("ActiveState=failed\nLoadState=loaded"), "failed");
        assert_eq!(
            state("LoadState=not-found\nActiveState=inactive"),
            "unknown",
            "a unit systemd does not know is unknown, not stopped"
        );
        assert_eq!(
            state("LoadState=loaded\nActiveState=maintenance"),
            "unknown"
        );
        assert_eq!(
            state("LoadState=loaded\nActiveState=activating\nSubState=auto-restart"),
            "failed",
            "waiting to be restarted after dying is failing"
        );
    }

    #[test]
    fn says_since_when_and_how_often_systemd_restarted_it() {
        // The clock at 100 s, the unit active since 40 s: a minute ago.
        let show = "LoadState=loaded\nActiveState=active\nSubState=running\nNRestarts=3\n\
                    ActiveEnterTimestampMonotonic=40000000\nInactiveEnterTimestampMonotonic=1";
        let s = shown(show, 1_000_000, 100_000_000);
        assert_eq!(
            (s.sub.as_str(), s.since, s.restarts),
            ("running", Some(940_000), 3)
        );
        let never = "LoadState=loaded\nActiveState=inactive\nInactiveEnterTimestampMonotonic=0";
        assert_eq!(shown(never, 1_000_000, 100_000_000).since, None);
    }

    #[test]
    fn reads_journal_entries_as_text_bytes_or_not_at_all() {
        let line = journal_line(
            r#"{"__REALTIME_TIMESTAMP":"1700000000123456","MESSAGE":"listening on :80  ","PRIORITY":"6"}"#,
        )
        .unwrap();
        assert_eq!(
            (line.at, line.text.as_str(), line.error),
            (1_700_000_000_123, "listening on :80", false)
        );
        let bytes =
            journal_line(r#"{"__REALTIME_TIMESTAMP":"1000","MESSAGE":[104,105],"PRIORITY":"3"}"#)
                .unwrap();
        assert_eq!((bytes.text.as_str(), bytes.error), ("hi", true));
        assert_eq!(
            journal_line(r#"{"__REALTIME_TIMESTAMP":"1000","MESSAGE":null}"#),
            None
        );
        assert_eq!(journal_line("not json"), None);
    }
}
