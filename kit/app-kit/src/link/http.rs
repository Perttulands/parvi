//! Plain HTTP/1.1 to the serving node, for what pages read and write over
//! HTTP (their areas' routes). No TLS: nodes listen on loopback or the
//! tailnet. A request for another node names it with `?node=`, and the
//! serving node passes it on.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How long an ordinary request may take. An action that runs longer on
/// the node (a service's units) passes its own deadline.
pub const ANSWER: Duration = Duration::from_secs(5);

/// A request that got no answer.
#[derive(Clone, Debug, PartialEq)]
pub struct NoAnswer {
    pub message: String,
    /// The request may have reached the node, so what it asked for may
    /// have happened: a write's outcome is unknown, not failed.
    pub maybe_done: bool,
}

impl std::fmt::Display for NoAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// `host:port` and the `Host` header for a node address.
fn address(base: &str) -> (String, String) {
    let host = base
        .trim()
        .trim_end_matches('/')
        .trim_start_matches("http://")
        .trim_start_matches("ws://")
        .to_string();
    let addr = if host.contains(':') {
        host.clone()
    } else {
        format!("{host}:80")
    };
    (addr, host)
}

/// Any request to the node: `body` is its content type and bytes (a Files
/// upload is not JSON). Answered `within`, or an error saying it was not.
pub async fn send(
    base: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<(&str, &[u8])>,
    within: Duration,
) -> Result<(u16, Vec<u8>), NoAnswer> {
    let (addr, host) = address(base);
    // Set once any of the request may have left: from then on, a write
    // that gets no answer may still have happened.
    let sent = AtomicBool::new(false);
    let fetch = async {
        let mut s = tokio::net::TcpStream::connect(&addr)
            .await
            .map_err(|e| e.to_string())?;
        sent.store(method != "GET", Ordering::Relaxed);
        let length = match body {
            Some((kind, b)) => format!("Content-Type: {kind}\r\nContent-Length: {}\r\n", b.len()),
            None if method == "GET" => String::new(),
            None => "Content-Length: 0\r\n".into(),
        };
        let extra: String = headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept-Encoding: identity\r\n{extra}{length}\r\n"
        );
        s.write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        if let Some((_, b)) = body {
            s.write_all(b).await.map_err(|e| e.to_string())?;
        }
        let mut all = Vec::new();
        s.read_to_end(&mut all).await.map_err(|e| e.to_string())?;
        parse(&all)
    };
    let answer = match tokio::time::timeout(within, fetch).await {
        Ok(a) => a,
        Err(_) => Err(format!("{NO_ANSWER} {} s", within.as_secs())),
    };
    answer.map_err(|message| NoAnswer {
        message,
        maybe_done: sent.load(Ordering::Relaxed),
    })
}

/// How a request that timed out begins its reason, so a caller can tell a
/// silence (the node may still be acting) from a refusal.
pub const NO_ANSWER: &str = "no answer in";

/// The node's `{error}` words for a failed answer, or its status.
pub fn error_text(status: u16, body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| match status {
            413 => "Too large".into(),
            0 => "Not reachable".into(),
            s => format!("Failed ({s})"),
        })
}

fn chunked(head: &str) -> bool {
    head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    })
}

fn parse(all: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let end = all
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no header end")?;
    let head = String::from_utf8_lossy(&all[..end]);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("no status")?;
    let body = &all[end + 4..];
    Ok((
        status,
        if chunked(&head) {
            dechunk(body)?
        } else {
            body.to_vec()
        },
    ))
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line = b.windows(2).position(|w| w == b"\r\n").ok_or("bad chunk")?;
        let size = usize::from_str_radix(
            String::from_utf8_lossy(&b[..line])
                .split(';')
                .next()
                .unwrap_or("")
                .trim(),
            16,
        )
        .map_err(|_| "bad chunk size")?;
        b = &b[line + 2..];
        if size == 0 {
            return Ok(out);
        }
        out.extend_from_slice(b.get(..size).ok_or("short chunk")?);
        b = b.get(size + 2..).ok_or("short chunk")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_plain_and_a_chunked_body() {
        assert_eq!(
            parse(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi").unwrap(),
            (200, b"hi".to_vec())
        );
        let chunked = b"HTTP/1.1 404 Not Found\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        assert_eq!(parse(chunked).unwrap(), (404, b"abcde".to_vec()));
    }
}
