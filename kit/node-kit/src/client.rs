//! The CLI's HTTP connection to a node. An id is routed through the local node.
use anyhow::{Context, Result, bail};
use axum::{
    body::Body,
    http::{Method, Request, header},
};
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::net::TcpStream;

/// An error returned by a node, retaining its code for CLI callers.
#[derive(Debug)]
pub struct NodeError {
    pub status: axum::http::StatusCode,
    pub code: String,
    pub message: String,
    pub findings: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn plain_text_peer_failures_keep_status_and_message() {
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(|| async {
                (axum::http::StatusCode::BAD_GATEWAY, "peer is offline")
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let error = NodeClient::new(Some(&format!("http://{address}")))
            .unwrap()
            .request(Method::GET, "/", None, None)
            .await
            .unwrap_err();
        let error = error.downcast_ref::<NodeError>().unwrap();
        assert_eq!(error.status, axum::http::StatusCode::BAD_GATEWAY);
        assert_eq!(error.message, "peer is offline");
        task.abort();
    }
}
impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "node HTTP {}: {}: {}",
            self.status, self.code, self.message
        )
    }
}
impl std::error::Error for NodeError {}

#[derive(Clone, Debug)]
pub struct NodeClient {
    authority: String,
    node: Option<String>,
}
/// A node id as `--node` takes it.
fn node_id(id: &str) -> Result<String> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!("invalid node id");
    }
    Ok(id.into())
}

impl NodeClient {
    pub fn websocket_url(&self) -> String {
        match &self.node {
            Some(node) => format!("ws://{}/ws?node={node}", self.authority),
            None => format!("ws://{}/ws", self.authority),
        }
    }
    pub fn new(node: Option<&str>) -> Result<Self> {
        let default = crate::address::local();
        match node {
            None => Ok(Self {
                authority: default,
                node: None,
            }),
            Some(v) if v.starts_with("http://") => {
                let uri: axum::http::Uri = v.parse().context("invalid node URL")?;
                let node = match uri.query() {
                    None => None,
                    Some(query) => Some(node_id(
                        query
                            .strip_prefix("node=")
                            .context("a node URL's only query is node=<id>")?,
                    )?),
                };
                if uri.path() != "/" {
                    bail!("node URL must have no path");
                }
                Ok(Self {
                    authority: uri
                        .authority()
                        .context("node URL needs a host")?
                        .as_str()
                        .to_owned(),
                    node,
                })
            }
            Some(v) if v.contains("://") => bail!("node URL must use http"),
            Some(v) => Ok(Self {
                authority: default,
                node: Some(node_id(v)?),
            }),
        }
    }
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        etag: Option<&str>,
    ) -> Result<(Value, String)> {
        let path = if let Some(node) = &self.node {
            format!(
                "{path}{}node={node}",
                if path.contains('?') { '&' } else { '?' }
            )
        } else {
            path.to_owned()
        };
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, &self.authority);
        if let Some(etag) = etag {
            builder = builder.header(header::IF_MATCH, etag);
        }
        let bytes = match body {
            Some(body) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                serde_json::to_vec(&body)?
            }
            None => Vec::new(),
        };
        let request = builder.body(Body::from(bytes))?;
        let stream = TcpStream::connect(&self.authority)
            .await
            .context("connecting to node")?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let response = sender.send_request(request).await?;
        let status = response.status();
        let tag = response
            .headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let bytes = response.into_body().collect().await?.to_bytes();
        if !status.is_success() {
            let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let plain = String::from_utf8_lossy(&bytes);
            return Err(NodeError {
                status,
                code: value["error"]["code"].as_str().unwrap_or("ERROR").into(),
                message: value["error"]["message"]
                    .as_str()
                    .unwrap_or(if plain.trim().is_empty() {
                        "request failed"
                    } else {
                        plain.trim()
                    })
                    .into(),
                findings: value["error"].get("findings").cloned(),
            }
            .into());
        }
        let value: Value = serde_json::from_slice(&bytes).context("invalid JSON from node")?;
        Ok((value, tag))
    }
}
