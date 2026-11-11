//! The Work module's HTTP route: one item with its family, read on demand
//! (`docs/work.md`, The seam).

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use super::poller::Work;

/// Work's routes, for the server to merge with the other modules'.
pub fn routes<S: Clone + Send + Sync + 'static>(work: Work) -> Router<S> {
    Router::new()
        .route("/api/work/item", get(item))
        .route("/api/work/items", post(create))
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
        Err(error) => {
            if let Some(not_found) = error.downcast_ref::<parvi_core::Error>()
                && not_found.code == "not_found"
            {
                return (StatusCode::NOT_FOUND, Json(json!({"error": not_found}))).into_response();
            }
            refuse(
                StatusCode::BAD_GATEWAY,
                parvi_work_contract::TRACKER_ERROR,
                error,
            )
        }
    }
}

async fn create(
    State(work): State<Work>,
    request: Result<Json<parvi_work_contract::Create>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                "bad_request",
                anyhow::anyhow!(error.to_string()),
            );
        }
    };
    match work.create(&request).await {
        Ok(reference) => Json(json!({"reference": reference})).into_response(),
        Err(error) => {
            let status = match error.code.as_str() {
                "unknown_project" | "bad_request" => StatusCode::BAD_REQUEST,
                _ => StatusCode::BAD_GATEWAY,
            };
            (status, Json(json!({"error": error}))).into_response()
        }
    }
}

fn refuse(status: StatusCode, code: &str, error: anyhow::Error) -> Response {
    (
        status,
        Json(json!({ "error": parvi_core::Error::new(code, format!("{error:#}")) })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn create_route_returns_the_work_error_shape() {
        let app: Router = routes(Work::default());
        let response = app
            .oneshot(
                Request::post("/api/work/items")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"project":"missing","title":"T","description":"D"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["code"], "unknown_project");
    }
}
