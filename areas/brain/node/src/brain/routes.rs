//! The Brain's HTTP routes.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use parvi_brain_contract::{Changes, Collections, Listing, Results, Save};

use super::{Brain, Refusal};

type Params = Query<HashMap<String, String>>;

/// The Brain's routes, for the server to merge with the other modules'.
pub fn routes<S: Clone + Send + Sync + 'static>(brain: Arc<Brain>) -> Router<S> {
    Router::new()
        .route("/api/brain/search", get(search))
        .route("/api/brain/page", get(page).put(save))
        .route("/api/brain/collections", get(collections))
        .route("/api/brain/collection", get(collection))
        .route("/api/brain/changes", get(changes))
        .with_state(brain)
}

fn answer<T: serde::Serialize>(read: Result<T, Refusal>) -> Response {
    match read {
        Ok(found) => Json(found).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

fn param<'a>(params: &'a HashMap<String, String>, name: &str) -> &'a str {
    params.get(name).map(String::as_str).unwrap_or("")
}

async fn search(State(brain): State<Arc<Brain>>, Query(params): Params) -> Response {
    let results = brain.search(param(&params, "q")).await;
    answer(results.map(|results| Results { results }))
}

async fn page(State(brain): State<Arc<Brain>>, Query(params): Params) -> Response {
    answer(brain.page(param(&params, "path")).await)
}

async fn save(
    State(brain): State<Arc<Brain>>,
    body: Result<Json<Save>, JsonRejection>,
) -> Response {
    let Ok(Json(save)) = body else {
        let why = "The body is not a page to save";
        return Refusal::new(StatusCode::BAD_REQUEST, why).into_response();
    };
    // A client can disconnect while git is writing. Keep the operation and
    // its corpus lock alive until it has committed or restored the page.
    match tokio::spawn(async move { brain.save(&save.path, &save.content, &save.summary).await })
        .await
    {
        Ok(result) => answer(result),
        Err(error) => Refusal::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Save stopped: {error}"),
        )
        .into_response(),
    }
}

async fn collections(State(brain): State<Arc<Brain>>) -> Response {
    let found = brain.collections().await;
    answer(found.map(|collections| Collections { collections }))
}

async fn collection(State(brain): State<Arc<Brain>>, Query(params): Params) -> Response {
    let found = brain.collection(param(&params, "name")).await;
    answer(found.map(|read| Listing {
        pages: read.found,
        error: read.error,
    }))
}

async fn changes(State(brain): State<Arc<Brain>>, Query(params): Params) -> Response {
    let limit = match param(&params, "limit") {
        "" => None,
        raw => match raw.parse::<usize>() {
            Ok(n) if n > 0 => Some(n),
            _ => {
                let why = "limit must be a positive whole number";
                return Refusal::new(StatusCode::BAD_REQUEST, why).into_response();
            }
        },
    };
    let found = brain.changes(limit).await;
    answer(found.map(|read| Changes {
        changes: read.found,
        error: read.error,
    }))
}
