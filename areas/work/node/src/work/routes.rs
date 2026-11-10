//! The Work module's HTTP route: one item with its family, read on demand
//! (`docs/work.md`, The seam).

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use super::poller::Work;

/// Work's routes, for the server to merge with the other modules'.
pub fn routes<S: Clone + Send + Sync + 'static>(work: Work) -> Router<S> {
    Router::new()
        .route("/api/work/item", get(item))
        .with_state(work)
}

/// `GET /api/work/item?ref=<reference>`: `{"item": …, "line": …}`, where
/// `line` names the item to an agent; 400 for a reference that does not
/// resolve, 502 for a tracker that failed to answer.
async fn item(State(work): State<Work>, Query(params): Query<HashMap<String, String>>) -> Response {
    let written = params.get("ref").map(String::as_str).unwrap_or("");
    let reference = match work.resolve(written).await {
        Ok(reference) => reference,
        Err(error) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                parvi_work_contract::BAD_REQUEST,
                error,
            );
        }
    };
    match work.item(&reference).await {
        Ok(item) => {
            let line = work.reference_line(&item).await;
            Json(json!({ "item": item, "line": line })).into_response()
        }
        Err(error) => refuse(
            StatusCode::BAD_GATEWAY,
            parvi_work_contract::TRACKER_ERROR,
            error,
        ),
    }
}

fn refuse(status: StatusCode, code: &str, error: anyhow::Error) -> Response {
    (
        status,
        Json(json!({ "error": parvi_core::Error::new(code, format!("{error:#}")) })),
    )
        .into_response()
}
