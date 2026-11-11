//! Running a probe: a short command whose stdout is the answer, with the
//! reported home as its environment and a time limit.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use super::Home;

/// How long `--version` or a local git query may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a fetch from origin may take.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// A command that runs as if in `home`, never asks on a terminal, and dies
/// with its caller. On Windows it opens no console window.
pub fn command(home: &Home, program: &Path) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    command
        .env("HOME", &home.dir)
        .env("PATH", home.path_var())
        .env("GIT_TERMINAL_PROMPT", "0")
        // Status refreshes the index only if it may lock it; it must not,
        // so the operator's own git never finds a stale lock.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// The command's stdout if it ran and succeeded within the probe limit.
pub async fn stdout(mut command: Command) -> Option<String> {
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A git query in `dir`: its trimmed stdout, if it worked.
pub async fn git(home: &Home, dir: &Path, args: &[&str]) -> Option<String> {
    let mut command = command(home, Path::new("git"));
    command.arg("-C").arg(dir).args(args);
    stdout(command).await.map(|out| out.trim().to_owned())
}

/// Fetches `dir`'s upstream. `Err` holds git's last line of complaint.
pub async fn fetch(home: &Home, dir: &Path) -> Result<(), String> {
    let mut command = command(home, Path::new("git"));
    command.arg("-C").arg(dir).args(["fetch", "--quiet"]);
    let output = match tokio::time::timeout(FETCH_TIMEOUT, command.output()).await {
        Err(_) => return Err("fetch took too long".into()),
        Ok(Err(error)) => return Err(format!("running git: {error}")),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or("fetch failed");
    Err(without_userinfo(line.trim()))
}

/// The text with any `user:password@` taken out of URLs in it, since a
/// remote's URL can carry a token.
fn without_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let host_end = tail
            .find(|c: char| c == '/' || c.is_whitespace())
            .unwrap_or(tail.len());
        rest = match tail[..host_end].rfind('@') {
            Some(user_end) => &tail[user_end + 1..],
            None => tail,
        };
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_credentials_out_of_urls() {
        assert_eq!(
            without_userinfo(
                "fatal: unable to access 'https://x:ghp_secret@github.com/a/b.git/': 403"
            ),
            "fatal: unable to access 'https://github.com/a/b.git/': 403"
        );
        assert_eq!(
            without_userinfo("ssh://git@host/x and more"),
            "ssh://host/x and more"
        );
        assert_eq!(without_userinfo("no url here"), "no url here");
    }
}
