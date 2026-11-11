//! Run and gate routes over the local coordinator.
use crate::routes::ApiError;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use parvi_missions::{AdmissionFailure, Coordinator};
use parvi_missions_contract::{self as contract, HumanPick, HumanVerdict, ResumeRequest, RunStart};
use serde_json::json;

pub fn routes(coordinator: Coordinator) -> Router {
    Router::new()
        .route("/api/runs", get(list).post(start))
        .route("/api/runs/{id}", get(read))
        .route("/api/runs/{id}/events", get(events))
        .route("/api/runs/{id}/mission", get(frozen))
        .route("/api/runs/{id}/seats", get(seats))
        .route("/api/runs/{id}/escalations", get(escalations))
        .route("/api/runs/{id}/evidence/attempts", get(attempts))
        .route("/api/runs/{id}/gates/{gate}/request", get(gate_request))
        .route("/api/runs/{id}/land", axum::routing::post(land))
        .route("/api/runs/{id}/abort", axum::routing::post(abort))
        .route("/api/runs/{id}/resume", axum::routing::post(resume))
        .route(
            "/api/runs/{id}/gates/{gate}/verdict",
            axum::routing::post(verdict),
        )
        .route(
            "/api/runs/{id}/gates/{gate}/pick",
            axum::routing::post(pick),
        )
        .with_state(coordinator)
}
type ResultResponse = Result<Response, ApiError>;
fn response<T: serde::Serialize>(result: Result<T, parvi_core::Error>) -> ResultResponse {
    Ok(Json(result?).into_response())
}
async fn list(
    State(c): State<Coordinator>,
    Query(q): Query<contract::RunsQuery>,
) -> ResultResponse {
    response(c.list(q.mission.as_deref(), q.needs.as_deref() == Some("you")))
}
async fn read(State(c): State<Coordinator>, Path(id): Path<String>) -> ResultResponse {
    response(c.run(&id))
}
async fn events(State(c): State<Coordinator>, Path(id): Path<String>) -> ResultResponse {
    response(c.events(&id))
}
async fn frozen(State(c): State<Coordinator>, Path(id): Path<String>) -> ResultResponse {
    let mission = c.frozen_mission(&id)?;
    Ok(Json(contract::MissionResponse {
        mission: contract::document::MissionDocument {
            mission,
            etag: None,
        },
    })
    .into_response())
}
async fn seats(State(c): State<Coordinator>, Path(id): Path<String>) -> ResultResponse {
    response(c.seats(&id))
}
async fn escalations(State(c): State<Coordinator>, Path(id): Path<String>) -> ResultResponse {
    response(c.escalations(&id))
}
async fn attempts(
    State(c): State<Coordinator>,
    Path(id): Path<String>,
    Query(q): Query<contract::AttemptsQuery>,
) -> ResultResponse {
    Ok(Json(json!({"attempts":c.attempts(&id, q.step.as_deref())?})).into_response())
}
async fn gate_request(
    State(c): State<Coordinator>,
    Path((id, gate)): Path<(String, String)>,
) -> ResultResponse {
    Ok(Json(json!({"request":c.gate_request(&id,&gate)?})).into_response())
}
async fn verdict(
    State(c): State<Coordinator>,
    Path((id, gate)): Path<(String, String)>,
    Json(mut body): Json<HumanVerdict>,
) -> ResultResponse {
    body.actor = "human:operator".into();
    c.record_verdict(&id, &gate, body)?;
    Ok((StatusCode::ACCEPTED, Json(contract::RunId { run_id: id })).into_response())
}
async fn pick(
    State(c): State<Coordinator>,
    Path((id, gate)): Path<(String, String)>,
    Json(mut body): Json<HumanPick>,
) -> ResultResponse {
    body.actor = "human:operator".into();
    c.record_pick(&id, &gate, body)?;
    Ok((StatusCode::ACCEPTED, Json(contract::RunId { run_id: id })).into_response())
}
async fn resume(
    State(c): State<Coordinator>,
    Path(id): Path<String>,
    Json(body): Json<ResumeRequest>,
) -> ResultResponse {
    response(c.resume(&id, body))
}
async fn abort(
    State(c): State<Coordinator>,
    Path(id): Path<String>,
    Json(body): Json<contract::Abort>,
) -> ResultResponse {
    response(c.abort(&id, &body.requested_by, &body.reason))
}
async fn land(
    State(c): State<Coordinator>,
    Path(id): Path<String>,
    Json(body): Json<contract::Land>,
) -> ResultResponse {
    Ok(Json(contract::Landed {
        landed: true,
        notes: c.land(&id, body.force)?,
    })
    .into_response())
}
async fn start(
    State(c): State<Coordinator>,
    headers: HeaderMap,
    Json(mut req): Json<RunStart>,
) -> ResultResponse {
    if !req.work.is_empty() {
        return Err(parvi_core::Error::new(
            "not_provided",
            "work reference resolution is not yet provided",
        )
        .into());
    }
    if req.mission.is_empty()
        || (req.input_card_id.is_empty() == req.formation_id.is_empty())
        || req.expected_rev <= 0
    {
        return Err(parvi_core::Error::new(
            "bad_request",
            "mission, one run target and expectedRev required",
        )
        .into());
    }
    req.expected_etag = headers
        .get(header::IF_MATCH)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .into();
    let result = if req.formation_id.is_empty() {
        c.start_mission(req)
    } else {
        c.start_formation(req)
    };
    match result {
        Ok(run_id) => Ok((StatusCode::ACCEPTED, Json(contract::RunId { run_id })).into_response()),
        Err(AdmissionFailure::Findings(findings)) => Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(contract::AdmissionRejection {
                error: parvi_core::Error::new(
                    contract::error::RUN_ADMISSION_FAILED,
                    "run admission failed",
                ),
                findings,
            }),
        )
            .into_response()),
        Err(AdmissionFailure::NotFound(message)) => {
            Err(parvi_core::Error::new("not_found", message).into())
        }
        Err(AdmissionFailure::Conflict) => {
            Err(parvi_core::Error::new("conflict", "mission changed").into())
        }
        Err(AdmissionFailure::Invalid(message)) => {
            Err(parvi_core::Error::new("bad_request", message).into())
        }
        Err(AdmissionFailure::Io(error)) => {
            Err(parvi_core::Error::new("io", error.to_string()).into())
        }
    }
}
