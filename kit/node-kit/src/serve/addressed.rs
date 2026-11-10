//! A request names the node it is for with `?node=<id>`, on any route and on
//! `/ws`; MCP calls name it with their `node` argument. This node serves only
//! itself: relaying to other nodes is not built, so a request for another is
//! refused as `unknown_node` rather than run here.

use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use parvi_core::{Error, NodeId};

use super::Failure;

/// The refusal of a request or call meant for `node`.
pub(crate) fn elsewhere(node: &str) -> Error {
    Error::new(
        "unknown_node",
        format!("node {node} is not this node, and this node reaches no other"),
    )
}

/// Lets through requests that name no node or this one.
pub(super) async fn guard(State(node): State<NodeId>, request: Request, next: Next) -> Response {
    match other(&request, &node) {
        Some(other) => Failure(StatusCode::MISDIRECTED_REQUEST, elsewhere(&other)).into_response(),
        None => next.run(request).await,
    }
}

/// The other node a request names, if it names one.
fn other(request: &Request, node: &NodeId) -> Option<String> {
    let Query(pairs) = Query::<Vec<(String, String)>>::try_from_uri(request.uri()).ok()?;
    pairs
        .into_iter()
        .find(|(key, value)| key == "node" && !value.is_empty() && value != node.as_str())
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(uri: &str) -> Option<String> {
        let request = Request::builder()
            .uri(uri)
            .body(Default::default())
            .unwrap();
        other(&request, &NodeId("here".into()))
    }

    #[test]
    fn only_another_node_named_in_the_query_is_other() {
        assert_eq!(named("/api/x"), None);
        assert_eq!(named("/api/x?node=here"), None);
        assert_eq!(named("/api/x?node="), None);
        assert_eq!(named("/api/x?path=node%3Dthere"), None);
        assert_eq!(named("/ws?node=there"), Some("there".into()));
        assert_eq!(
            named("/api/x?a=1&node=here&node=th%65re"),
            Some("there".into())
        );
    }
}
