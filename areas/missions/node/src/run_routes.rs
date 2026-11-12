//! Run and gate routes over the local coordinator.
use crate::{Missions, routes::ApiError};
use axum::{
    Json, Router,
    extract::{
        FromRef, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use parvi_mission_engine::{AdmissionFailure, Coordinator};
use parvi_missions_contract::{self as contract, HumanPick, HumanVerdict, ResumeRequest, RunStart};
use serde_json::json;

impl FromRef<Missions> for Coordinator {
    fn from_ref(m: &Missions) -> Self {
        m.coordinator.clone()
    }
}

pub fn routes(missions: Missions) -> Router {
    Router::new()
        .route("/api/runs", get(list).post(start))
        .route("/api/runs/{id}", get(read))
        .route("/api/runs/{id}/events", get(events))
        .route("/api/runs/{id}/mission", get(frozen))
        .route("/api/runs/{id}/seats", get(seats))
        .route("/api/runs/{id}/escalations", get(escalations))
        .route("/api/runs/{id}/evidence", get(evidence))
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
        .with_state(missions)
}
type ResultResponse = Result<Response, ApiError>;
fn bad_json(e: JsonRejection) -> ApiError {
    parvi_core::Error::new("bad_request", e.body_text()).into()
}
fn response<T: serde::Serialize>(result: Result<T, parvi_core::Error>) -> ResultResponse {
    Ok(Json(result?).into_response())
}
/// Runs a coordinator call that may wait on seats (a tmux command, a seat
/// ending) off the async workers: such a call drives the node's runtime from
/// its own thread, and doing that from a worker can stall the whole node.
async fn off<T: Send + 'static>(
    c: Coordinator,
    call: impl FnOnce(Coordinator) -> T + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(move || call(c))
        .await
        .map_err(|e| parvi_core::Error::new("internal", e.to_string()).into())
}
async fn list(
    State(c): State<Coordinator>,
    q: Result<Query<contract::RunsQuery>, QueryRejection>,
) -> ResultResponse {
    let Query(q) = q.map_err(|e| parvi_core::Error::new("bad_request", e.body_text()))?;
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
    let seats = off(c, move |c| c.seats(&id))
        .await??
        .into_iter()
        .map(|seat| contract::AgentSeat {
            dispatch_id: seat.dispatch_id,
            slot_id: seat.slot_id,
            node: seat.node,
            session: seat.session,
            pane: seat.pane,
            session_name: seat.session_name,
            outcome: seat.outcome,
            available: seat.available,
            waiting: seat.waiting.map(|wait| contract::SeatWait {
                state: wait.state,
                detail: wait.detail,
                since: wait.since,
                seq: wait.seq,
            }),
        })
        .collect::<Vec<_>>();
    Ok(Json(seats).into_response())
}
async fn escalations(State(c): State<Coordinator>, Path(id): Path<String>) -> ResultResponse {
    response(c.escalations(&id))
}
async fn evidence(
    State(c): State<Coordinator>,
    Path(id): Path<String>,
    q: Result<Query<contract::AttemptsQuery>, QueryRejection>,
) -> ResultResponse {
    let Query(q) = q.map_err(|e| parvi_core::Error::new("bad_request", e.body_text()))?;
    Ok(Json(json!({"evidence": c.evidence(&id, q.step.as_deref())?})).into_response())
}
async fn attempts(
    State(c): State<Coordinator>,
    Path(id): Path<String>,
    q: Result<Query<contract::AttemptsQuery>, QueryRejection>,
) -> ResultResponse {
    let Query(q) = q.map_err(|e| parvi_core::Error::new("bad_request", e.body_text()))?;
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
    body: Result<Json<HumanVerdict>, JsonRejection>,
) -> ResultResponse {
    let Json(mut body) = body.map_err(bad_json)?;
    if body.actor.trim().is_empty() {
        body.actor = "human:operator".into();
    }
    let run = id.clone();
    off(c, move |c| c.record_verdict(&run, &gate, body)).await??;
    Ok((StatusCode::ACCEPTED, Json(contract::RunId { run_id: id })).into_response())
}
async fn pick(
    State(c): State<Coordinator>,
    Path((id, gate)): Path<(String, String)>,
    body: Result<Json<HumanPick>, JsonRejection>,
) -> ResultResponse {
    let Json(mut body) = body.map_err(bad_json)?;
    if body.actor.trim().is_empty() {
        body.actor = "human:operator".into();
    }
    let run = id.clone();
    off(c, move |c| c.record_pick(&run, &gate, body)).await??;
    Ok((StatusCode::ACCEPTED, Json(contract::RunId { run_id: id })).into_response())
}
async fn resume(
    State(c): State<Coordinator>,
    Path(id): Path<String>,
    body: Result<Json<ResumeRequest>, JsonRejection>,
) -> ResultResponse {
    let Json(body) = body.map_err(bad_json)?;
    response(off(c, move |c| c.resume(&id, body)).await?)
}
async fn abort(
    State(c): State<Coordinator>,
    Path(id): Path<String>,
    body: Result<Json<contract::Abort>, JsonRejection>,
) -> ResultResponse {
    let Json(body) = body.map_err(bad_json)?;
    response(off(c, move |c| c.abort(&id, &body.requested_by, &body.reason)).await?)
}
async fn land(
    State(c): State<Coordinator>,
    Path(id): Path<String>,
    body: Result<Json<contract::Land>, JsonRejection>,
) -> ResultResponse {
    let Json(body) = body.map_err(bad_json)?;
    Ok(Json(contract::Landed {
        landed: true,
        notes: off(c, move |c| c.land(&id, body.force)).await??,
    })
    .into_response())
}
async fn start(
    State(m): State<Missions>,
    headers: HeaderMap,
    body: Result<Json<RunStart>, JsonRejection>,
) -> ResultResponse {
    let Json(mut req) = body.map_err(|e| parvi_core::Error::new("bad_request", e.body_text()))?;
    if !req.work.is_empty() {
        req.work = m.resolver.resolve(&req.work).await?.to_string();
    }
    // Only the local environment can receive seats here.
    req.known_devices = Some(Vec::new());
    let c = m.coordinator;
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
    let result = off(c, move |c| {
        if req.formation_id.is_empty() {
            c.start_mission(req)
        } else {
            c.start_formation(req)
        }
    })
    .await?;
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
