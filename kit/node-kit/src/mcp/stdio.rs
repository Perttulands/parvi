//! `parvi mcp`: MCP over stdio for agents that start their servers as
//! processes. It relays each line to the local node's `/mcp` and prints the
//! answer, so the node serves and logs every call either way.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use super::http::PATH;

/// Relays stdin to the node at `node` (`addr:port`) until stdin ends.
pub async fn stdio(node: &str) -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let answer = match post(node, &line).await {
            Ok(answer) => answer,
            Err(error) => failed(&line, &format!("parvi node at {node}: {error:#}")),
        };
        if let Some(answer) = answer {
            stdout.write_all(answer.trim_end().as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

/// The node's answer to one message: `None` for a notification's 202.
async fn post(node: &str, message: &str) -> Result<Option<String>> {
    let mut stream = TcpStream::connect(node).await.context("connecting")?;
    let request = format!(
        "POST {PATH} HTTP/1.1\r\nHost: {node}\r\nUser-Agent: parvi-mcp-stdio\r\n\
         Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{message}",
        message.len()
    );
    stream.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    let response = String::from_utf8(response).context("the answer is not UTF-8")?;
    let (head, body) = response.split_once("\r\n\r\n").context("no answer")?;
    let status = head.split(' ').nth(1).unwrap_or("");
    match status {
        "200" => Ok(Some(body.to_owned())),
        "202" => Ok(None),
        _ => bail!("answered {status}: {}", body.trim()),
    }
}

/// An error answer to `message` if it was a request, so its caller is not
/// left waiting.
fn failed(message: &str, error: &str) -> Option<String> {
    let id = serde_json::from_str::<Value>(message)
        .ok()?
        .get("id")?
        .clone();
    let answer =
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": error } });
    Some(answer.to_string())
}
