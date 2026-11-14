//! Files' HTTP routes. Every request names a node and a path on it; this node
//! answers for itself. Paths are the node's own absolute paths.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::StatusCode;
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use parvi_files_contract::{At, DropAt, Dropped, Job, Move, NodeRoots, SaveAt, Search};

use super::Files;
use super::disk::{self, Listing};
use super::drops::Drops;
use super::error::FilesError;

/// The most one read or save carries.
pub const MAX_BYTES: u64 = 64 << 20;

pub fn routes(files: Arc<Files>, drops: Arc<Drops>) -> Router {
    Router::new()
        .route("/api/files/roots", get(roots))
        .route("/api/files/recent", get(recent))
        .route("/api/files/stat", post(stat))
        .route("/api/files/find", get(find))
        .route("/api/files/diff", get(diff))
        .route("/api/files/search", get(search))
        .route("/api/files/bridge/start", post(bridge_start))
        .route("/api/files/bridge/events", get(bridge_events))
        .route("/api/files/folder", get(list).post(create_folder))
        .route(
            "/api/files/file",
            get(read).put(save).post(create_file).delete(delete),
        )
        .route("/api/files/rename", post(rename))
        .layer(DefaultBodyLimit::max(MAX_BYTES as usize))
        .with_state(files)
        .merge(drop_routes(drops))
        .layer(axum::middleware::map_response(coded_rejection))
}

type Shared = State<Arc<Files>>;

/// This node's id and roots: where a browser starts.
async fn roots(State(files): Shared) -> Json<NodeRoots> {
    Json(NodeRoots {
        node: files.node.clone(),
        roots: files.roots.list(),
    })
}

#[derive(serde::Deserialize)]
struct NodeQuery {
    #[serde(default)]
    node: String,
}

/// What changed under the roots, or with `path` under that folder.
#[derive(serde::Deserialize)]
struct RecentQuery {
    #[serde(default)]
    node: String,
    path: Option<String>,
}

async fn recent(
    State(files): Shared,
    Query(at): Query<RecentQuery>,
) -> Result<Json<parvi_files_contract::Recent>, FilesError> {
    let result = blocking(&files, &at.node, move |f| f.recent(at.path.as_deref())).await?;
    Ok(Json(result))
}

async fn stat(
    State(files): Shared,
    Query(at): Query<NodeQuery>,
    Json(asked): Json<parvi_files_contract::Stat>,
) -> Result<Json<Vec<Option<u64>>>, FilesError> {
    Ok(Json(
        blocking(&files, &at.node, move |f| Ok(f.stat(&asked.paths))).await?,
    ))
}

async fn bridge_start(
    State(files): Shared,
    Json(start): Json<super::bridge::Start>,
) -> Result<StatusCode, FilesError> {
    super::bridge::start(files, start)?;
    Ok(StatusCode::ACCEPTED)
}

async fn bridge_events(
    State(files): Shared,
    Query(job): Query<Job>,
) -> Result<Response, FilesError> {
    if !job.node.is_empty() && job.node != files.node {
        return Err(FilesError::OtherNode(job.node));
    }
    let receiver = super::bridge::watch(&files, &job.id).ok_or(FilesError::NotFound)?;
    let stream = futures_util::stream::unfold(
        (receiver, true, false),
        |(mut receiver, first, done)| async move {
            if done {
                return None;
            }
            if !first && receiver.changed().await.is_err() {
                return None;
            }
            let value = receiver.borrow_and_update().clone();
            let finished = value.state != "running";
            let text = format!("data: {}\n\n", serde_json::to_string(&value).ok()?);
            Some((
                Ok::<_, std::convert::Infallible>(text),
                (receiver, false, finished),
            ))
        },
    );
    Ok((
        [
            (CONTENT_TYPE, "text/event-stream"),
            (CACHE_CONTROL, "no-store"),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response())
}

async fn diff(
    State(files): Shared,
    Query(at): Query<At>,
) -> Result<Json<super::diff::Diff>, FilesError> {
    let result = blocking(&files, &at.node, move |f| {
        super::diff::read(&f.roots, &at.path)
    })
    .await?;
    Ok(Json(result))
}

async fn find(
    State(files): Shared,
    Query(at): Query<Search>,
) -> Result<Json<super::find::Matches>, FilesError> {
    let result = blocking(&files, &at.node, move |f| {
        if at.path.is_empty() {
            let near = (!at.near.is_empty()).then_some(at.near.as_str());
            Ok(f.locate(&at.q, near))
        } else {
            f.find(&at.path, &at.q, false)
        }
    })
    .await?;
    Ok(Json(result))
}

async fn search(
    State(files): Shared,
    Query(at): Query<Search>,
) -> Result<Json<super::find::Matches>, FilesError> {
    let result = blocking(&files, &at.node, move |f| {
        super::find::search(&f.roots, &at.path, &at.q, true)
    })
    .await?;
    Ok(Json(result))
}

async fn list(State(files): Shared, Query(at): Query<At>) -> Result<Json<Listing>, FilesError> {
    let listing = blocking(&files, &at.node, move |f| disk::list(&f.roots, &at.path)).await?;
    Ok(Json(listing))
}

/// A file's bytes. Served so a page can show a picture, and never run one:
/// the type is only ever an image's or plain bytes, and the policy sandboxes
/// anything opened on its own.
async fn read(State(files): Shared, Query(at): Query<At>) -> Result<Response, FilesError> {
    let kind = content_type(&at.path);
    let bytes = blocking(&files, &at.node, move |f| {
        disk::read(&f.roots, &at.path, MAX_BYTES)
    })
    .await?;
    let headers = [
        (CONTENT_TYPE, HeaderValue::from_static(kind)),
        (CACHE_CONTROL, HeaderValue::from_static("no-store")),
        (CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox")),
        (
            axum::http::header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
    ];
    Ok((headers, bytes).into_response())
}

async fn save(
    State(files): Shared,
    Query(at): Query<SaveAt>,
    body: Bytes,
) -> Result<StatusCode, FilesError> {
    blocking(&files, &at.node, move |f| {
        disk::save(&f.roots, &at.path, &body, at.base.as_deref())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A new file, empty or with the body's bytes; never over an existing one.
async fn create_file(
    State(files): Shared,
    Query(at): Query<At>,
    body: Bytes,
) -> Result<StatusCode, FilesError> {
    blocking(&files, &at.node, move |f| {
        disk::upload(&f.roots, &at.path, &body)
    })
    .await?;
    Ok(StatusCode::CREATED)
}

async fn create_folder(
    State(files): Shared,
    Query(at): Query<At>,
) -> Result<StatusCode, FilesError> {
    blocking(&files, &at.node, move |f| {
        disk::create_folder(&f.roots, &at.path)
    })
    .await?;
    Ok(StatusCode::CREATED)
}

async fn delete(State(files): Shared, Query(at): Query<At>) -> Result<StatusCode, FilesError> {
    blocking(&files, &at.node, move |f| disk::delete(&f.roots, &at.path)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn rename(State(files): Shared, Query(at): Query<Move>) -> Result<StatusCode, FilesError> {
    blocking(&files, &at.node, move |f| {
        disk::rename(&f.roots, &at.path, &at.to)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Runs a disk operation off the async threads, once the request is known to
/// be for this node.
async fn blocking<T: Send + 'static>(
    files: &Arc<Files>,
    node: &str,
    work: impl FnOnce(&Files) -> Result<T, FilesError> + Send + 'static,
) -> Result<T, FilesError> {
    if !node.is_empty() && node != files.node {
        return Err(FilesError::OtherNode(node.to_owned()));
    }
    let files = files.clone();
    tokio::task::spawn_blocking(move || work(&files))
        .await
        .map_err(|error| FilesError::Io(std::io::Error::other(error)))?
}

/// The type a picture needs to show in an `img`; everything else is bytes.
fn content_type(path: &str) -> &'static str {
    let extension = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match extension.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

impl IntoResponse for FilesError {
    fn into_response(self) -> Response {
        let status = match &self {
            FilesError::Outside | FilesError::Root | FilesError::Permission => {
                StatusCode::FORBIDDEN
            }
            FilesError::NotFound => StatusCode::NOT_FOUND,
            FilesError::OtherNode(_) => StatusCode::MISDIRECTED_REQUEST,
            FilesError::Exists => StatusCode::CONFLICT,
            FilesError::Changed => StatusCode::PRECONDITION_FAILED,
            FilesError::NotFolder | FilesError::NotFile => StatusCode::BAD_REQUEST,
            FilesError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            FilesError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        parvi_node_kit::serve::Failure(status, parvi_core::Error::from(self)).into_response()
    }
}

fn drop_routes(drops: Arc<Drops>) -> Router {
    Router::new()
        .route("/api/drops", post(drop_file))
        .layer(DefaultBodyLimit::max(super::drops::MAX_BYTES))
        .with_state(drops)
}

async fn drop_file(
    State(drops): State<Arc<Drops>>,
    Query(at): Query<DropAt>,
    body: Bytes,
) -> Response {
    let result = tokio::task::spawn_blocking(move || drops.write(&at.name, &at.user, &body)).await;
    match result {
        Ok(Ok(path)) => (StatusCode::CREATED, Json(Dropped { path })).into_response(),
        Ok(Err(error)) => parvi_node_kit::serve::Failure(
            StatusCode::BAD_REQUEST,
            parvi_core::Error::new("drop_failed", format!("{error:#}")),
        )
        .into_response(),
        Err(error) => parvi_node_kit::serve::Failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            parvi_core::Error::new("io_error", error.to_string()),
        )
        .into_response(),
    }
}

/// Axum's extractor failures also use the node's coded error envelope.
async fn coded_rejection(response: Response) -> Response {
    let status = response.status();
    if !status.is_client_error()
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"))
    {
        return response;
    }
    let message = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_else(|_| "Request could not be read".into());
    let code = if status == StatusCode::PAYLOAD_TOO_LARGE {
        "too_large"
    } else {
        "bad_request"
    };
    parvi_node_kit::serve::Failure(status, parvi_core::Error::new(code, message)).into_response()
}
