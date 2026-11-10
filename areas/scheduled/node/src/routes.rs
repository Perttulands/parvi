//! CHROTE's scheduled-task routes, envelope and guards (`api/scheduled.go`):
//! `{success, data, timestamp}` or `{success: false, error: {code,
//! message}, timestamp}`; every mutation carries `X-Chrote-Intent:
//! scheduled-task`, and a create or patch is JSON.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use parvi_core::Error as Coded;
use parvi_scheduled_contract::{
    BAD_REQUEST, CreateRequest, Envelope, FORBIDDEN, INTENT, INTENT_VALUE, NOT_FOUND, PATH,
    PatchRequest, Target, TargetRequest, UNSUPPORTED_MEDIA_TYPE,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::service::{Create, Error, Patch, Scheduled};

pub fn routes(s: Arc<Scheduled>) -> Router {
    Router::new()
        .route(PATH, get(list).post(create))
        .route(
            &format!("{PATH}/{{id}}"),
            get(read).patch(patch).delete(delete),
        )
        .route(&format!("{PATH}/{{id}}/{{action}}"), post(act))
        .with_state(s)
}

fn stamp() -> String {
    jiff::Timestamp::now()
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn success(data: Value) -> Response {
    axum::Json(Envelope {
        success: true,
        data: Some(data),
        error: None,
        timestamp: stamp(),
    })
    .into_response()
}

fn refuse(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    let body = Envelope {
        success: false,
        data: None,
        error: Some(Coded::new(code, message)),
        timestamp: stamp(),
    };
    (status, axum::Json(body)).into_response()
}

pub(crate) fn failed(e: Error) -> Response {
    let status = match &e {
        Error::NotFound => StatusCode::NOT_FOUND,
        Error::Invalid(_) | Error::TargetNotFound(_) => StatusCode::BAD_REQUEST,
        Error::Conflict(_) => StatusCode::CONFLICT,
        Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let Coded { code, message } = e.into();
    refuse(status, &code, message)
}

/// The mutation guard: the intent header, and JSON where a body matters.
fn guarded(headers: &HeaderMap, json_body: bool) -> Option<Response> {
    if headers.get(INTENT).and_then(|v| v.to_str().ok()) != Some(INTENT_VALUE) {
        return Some(refuse(
            StatusCode::FORBIDDEN,
            FORBIDDEN,
            "scheduled task mutations require X-Chrote-Intent: scheduled-task",
        ));
    }
    let media = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase());
    if json_body && media.as_deref() != Some("application/json") {
        return Some(refuse(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            UNSUPPORTED_MEDIA_TYPE,
            "scheduled task mutations require Content-Type: application/json",
        ));
    }
    None
}

/// Refuses the retired single `target` and any socket: the node resolves
/// sockets itself. The refusal is a bad request's message.
fn targets(retired: &Option<Value>, many: Vec<TargetRequest>) -> Result<Vec<Target>, String> {
    if retired.is_some() {
        return Err("target is retired; use targets instead".into());
    }
    if many.iter().any(|t| !t.socket.is_empty()) {
        return Err("target socket is not accepted; the node resolves sockets itself".into());
    }
    Ok(many
        .into_iter()
        .map(|t| Target {
            session_name: t.session_name,
            unix_user: t.unix_user,
        })
        .collect())
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    serde_json::from_slice(body).map_err(|e| format!("invalid scheduled task JSON: {e}"))
}

fn bad(message: String) -> Response {
    refuse(StatusCode::BAD_REQUEST, BAD_REQUEST, message)
}

fn actor(body: &[u8]) -> String {
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct Actor {
        #[serde(default)]
        updated_by: String,
    }
    serde_json::from_slice::<Actor>(body)
        .unwrap_or_default()
        .updated_by
        .trim()
        .to_string()
}

async fn list(State(s): State<Arc<Scheduled>>) -> Response {
    match s.list() {
        Ok(tasks) => success(json!({ "tasks": tasks })),
        Err(e) => failed(e),
    }
}

async fn read(State(s): State<Arc<Scheduled>>, Path(id): Path<String>) -> Response {
    match s.get(&id) {
        Ok(task) => success(json!({ "task": task })),
        Err(e) => failed(e),
    }
}

async fn create(State(s): State<Arc<Scheduled>>, headers: HeaderMap, body: Bytes) -> Response {
    if let Some(no) = guarded(&headers, true) {
        return no;
    }
    let b: CreateRequest = match parse(&body) {
        Ok(b) => b,
        Err(m) => return bad(m),
    };
    let targets = match targets(&b.target, b.targets) {
        Ok(t) => t,
        Err(m) => return bad(m),
    };
    let request = Create {
        name: b.name,
        prompt: b.prompt,
        targets,
        schedule: b.schedule,
        enabled: b.enabled,
        paused: b.paused,
        created_by: b.created_by,
        updated_by: b.updated_by,
    };
    match s.create(request).await {
        Ok(task) => success(json!({ "task": task })),
        Err(e) => failed(e),
    }
}

async fn patch(
    State(s): State<Arc<Scheduled>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(no) = guarded(&headers, true) {
        return no;
    }
    let b: PatchRequest = match parse(&body) {
        Ok(b) => b,
        Err(m) => return bad(m),
    };
    let targets = match b.targets {
        Some(many) => match targets(&b.target, many) {
            Ok(t) => Some(t),
            Err(m) => return bad(m),
        },
        None => match targets(&b.target, vec![]) {
            Ok(_) => None,
            Err(m) => return bad(m),
        },
    };
    let request = Patch {
        name: b.name,
        prompt: b.prompt,
        targets,
        schedule: b.schedule,
        enabled: b.enabled,
        paused: b.paused,
        updated_by: b.updated_by,
    };
    match s.patch(&id, request).await {
        Ok(task) => success(json!({ "task": task })),
        Err(e) => failed(e),
    }
}

async fn delete(
    State(s): State<Arc<Scheduled>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(no) = guarded(&headers, false) {
        return no;
    }
    match s.delete(&id) {
        Ok(()) => success(json!({ "deleted": id })),
        Err(e) => failed(e),
    }
}

async fn act(
    State(s): State<Arc<Scheduled>>,
    Path((id, action)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(no) = guarded(&headers, false) {
        return no;
    }
    let who = actor(&body);
    let done = match action.as_str() {
        "pause" => s.pause(&id, &who).map(|task| json!({ "task": task })),
        "resume" => s
            .resume(&id, &who)
            .await
            .map(|task| json!({ "task": task })),
        "run-now" => s
            .run_now(&id, &who)
            .await
            .map(|(task, run)| json!({ "task": task, "run": run })),
        _ => return refuse(StatusCode::NOT_FOUND, NOT_FOUND, "no such action"),
    };
    match done {
        Ok(data) => success(data),
        Err(e) => failed(e),
    }
}
