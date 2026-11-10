//! `parvi hook <url> [--then <command…>] [payload]`: what a program's own
//! hook runs to tell the node something. It posts the payload to `url` on
//! this machine's node and exits 0 whatever happens: no notice is worth
//! stalling the program that sent it. The payload is the last argument, or
//! else stdin. After `--then` comes another command the hook should also
//! run, which is run with the payload as its last argument, even when the
//! node is gone.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// How long a hook may take to report.
const REPORT_WITHIN: Duration = Duration::from_secs(2);

/// The most of a payload that is read.
pub const MAX_PAYLOAD: usize = 16 * 1024;

/// The word before the command run after the report.
pub const THEN: &str = "--then";

/// The CLI word that runs [`report`].
pub const VERB: &str = "hook";

/// Posts one report, then runs the command after `--then`, if any.
pub fn report(args: &[String]) {
    let [url, rest @ ..] = args else {
        return;
    };
    let (then, last) = match rest {
        [flag, then @ .., payload] if flag == THEN => (then, Some(payload)),
        [.., payload] => (&[][..], Some(payload)),
        [] => (&[][..], None),
    };
    let theirs = then.split_first().map(|(program, args)| {
        std::process::Command::new(program)
            .args(args)
            .args(last)
            .spawn()
            .with_context(|| format!("running {program}"))
    });
    let payload = match last {
        Some(payload) => payload.as_bytes().to_vec(),
        None => {
            use std::io::IsTerminal;
            let mut payload = Vec::new();
            if !std::io::stdin().is_terminal() {
                let _ = std::io::stdin()
                    .take(MAX_PAYLOAD as u64)
                    .read_to_end(&mut payload);
            }
            payload
        }
    };
    if let Err(error) = post(url, &payload) {
        eprintln!("parvi {VERB}: {error:#}");
    }
    match theirs {
        Some(Ok(mut child)) => {
            let _ = child.wait();
        }
        Some(Err(error)) => eprintln!("parvi {VERB}: {error:#}"),
        None => {}
    }
}

/// One plain HTTP POST, bounded by [`REPORT_WITHIN`].
fn post(url: &str, body: &[u8]) -> Result<()> {
    let rest = url.strip_prefix("http://").context("not an http URL")?;
    let (host, path) = rest.split_at(rest.find('/').context("no path")?);
    let addr = host
        .to_socket_addrs()?
        .next()
        .context("no address for the node")?;
    let mut stream = TcpStream::connect_timeout(&addr, REPORT_WITHIN)?;
    stream.set_write_timeout(Some(REPORT_WITHIN))?;
    stream.set_read_timeout(Some(REPORT_WITHIN))?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    let mut answer = [0u8; 64];
    let n = stream.read(&mut answer)?;
    let status = std::str::from_utf8(&answer[..n]).unwrap_or_default();
    if !status.starts_with("HTTP/1.1 2") {
        bail!("the node answered {}", status.lines().next().unwrap_or(""));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The other command runs with the payload, even when the node is gone.
    /// Unix only: the command here is `sh`.
    #[cfg(unix)]
    #[test]
    fn the_command_after_then_gets_the_payload_when_the_node_is_gone() {
        let dir = std::env::temp_dir().join(format!("parvi-hook-then-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("theirs");
        let script = format!("printf %s \"$1\" > '{}'", out.display());
        report(&[
            "http://127.0.0.1:1/api/report?event=finished".into(),
            THEN.into(),
            "sh".into(),
            "-c".into(),
            script,
            "sh".into(),
            r#"{"type":"agent-turn-complete"}"#.into(),
        ]);
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            r#"{"type":"agent-turn-complete"}"#
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
