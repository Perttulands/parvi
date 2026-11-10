//! MCP's streamable HTTP transport, on the node's own port: one JSON-RPC
//! message per POST to `/mcp`, answered with JSON. The server opens no
//! streams of its own, so GET is refused, as the transport allows.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::Value;

use super::Mcp;
use super::rpc::{self, Caller};

/// The path the endpoint answers on.
pub const PATH: &str = "/mcp";

/// The largest message taken: a 64 KiB text with room for JSON's escapes.
const MAX_MESSAGE: usize = 1024 * 1024;

/// Set by Tailscale Funnel on requests from the public internet.
const FUNNEL: &str = "tailscale-funnel-request";

pub(super) fn routes(mcp: Arc<Mcp>) -> Router {
    Router::new()
        .route(PATH, post(message).get(no_stream).delete(no_stream))
        .with_state(mcp)
}

async fn message(State(mcp): State<Arc<Mcp>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    // There is no public endpoint (VISION.md): a Funnel request gets
    // nothing, whatever the node's serve settings.
    if parts.headers.contains_key(FUNNEL) {
        return (StatusCode::FORBIDDEN, "the public MCP door is not open").into_response();
    }
    let Ok(body) = axum::body::to_bytes(body, MAX_MESSAGE).await else {
        return (StatusCode::PAYLOAD_TOO_LARGE, "the message is too large").into_response();
    };
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0);
    let caller = Caller {
        who: who(peer, &parts.headers),
    };
    let answer = match serde_json::from_slice::<Value>(&body) {
        Ok(message) => rpc::answer(&mcp, &caller, &message).await,
        Err(error) => Some(rpc::unreadable(&error)),
    };
    match answer {
        Some(answer) => (
            [(header::CONTENT_TYPE, "application/json")],
            Body::from(answer.to_string()),
        )
            .into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

async fn no_stream() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response()
}

/// The caller as the call log names it: its address and what it says it is.
fn who(peer: Option<SocketAddr>, headers: &HeaderMap) -> String {
    let peer = peer.map_or_else(|| "?".to_owned(), |peer| peer.to_string());
    match headers
        .get(header::USER_AGENT)
        .and_then(|ua| ua.to_str().ok())
    {
        Some(agent) => format!("{peer} {agent}"),
        None => peer,
    }
}
