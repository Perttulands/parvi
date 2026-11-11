//! HTTP authoring routes. Writes publish after the store commits them.
use super::Missions;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        Path, State,
        rejection::{BytesRejection, JsonRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use parvi_mission_engine::{Harnesses, validate};
use parvi_missions_contract::patch;
use parvi_missions_contract::{LayoutPatch, NotePatch, Precondition};
use serde_json::{Value, json};

pub fn routes(m: Missions) -> Router {
    Router::new()
        .route("/api/missions", get(list).post(create))
        .route("/api/characters", get(characters).post(create_character))
        .route(
            "/api/characters/{id}",
            axum::routing::put(put_character).delete(delete_character),
        )
        .route("/api/gate-profiles", get(gate_profiles))
        .route("/api/missions/{id}", get(read).patch(edit).delete(delete))
        .route("/api/missions/{id}/validation", get(validation))
        .route("/api/missions/{id}/notes", get(notes).patch(edit_notes))
        .route("/api/missions/{id}/layout", get(layout).patch(edit_layout))
        .with_state(m)
}
type ResultResponse = Result<Response, ApiError>;
type JsonBody<T> = Result<Json<T>, JsonRejection>;
fn body<T>(input: JsonBody<T>, code: &str) -> Result<T, ApiError> {
    input.map(|Json(value)| value).map_err(|error| {
        ApiError(
            if code == "invalid_character" {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::BAD_REQUEST
            },
            code.into(),
            error.body_text(),
        )
    })
}

pub(crate) struct ApiError(StatusCode, String, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(parvi_missions_contract::ErrorResponse {
                error: parvi_core::Error::new(self.1, self.2),
            }),
        )
            .into_response()
    }
}
impl From<parvi_core::Error> for ApiError {
    fn from(e: parvi_core::Error) -> Self {
        let status = match e.code.as_str() {
            "not_found" => StatusCode::NOT_FOUND,
            "note_author_mismatch" => StatusCode::FORBIDDEN,
            "note_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
            "conflict"
            | "already_exists"
            | "input_occupied"
            | "duplicate_connection"
            | "not_pending"
            | "character_exists" => StatusCode::CONFLICT,
            "precondition_required" => StatusCode::PRECONDITION_REQUIRED,
            "invalid_character"
            | "invalid_code_gate_profile"
            | "invalid_slot_settings"
            | "invalid_definition_source"
            | "unsupported_schema"
            | "self_wire" => StatusCode::UNPROCESSABLE_ENTITY,
            "io" => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        Self(status, e.code, e.message)
    }
}
fn bad(s: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "bad_request".into(), s.into())
}
fn ok(value: Value) -> Response {
    Json(value).into_response()
}
fn tagged(value: Value, etag: &str) -> Response {
    let mut r = ok(value);
    if let Ok(h) = etag.parse() {
        r.headers_mut().insert(header::ETAG, h);
    }
    r
}
fn etag(h: &HeaderMap) -> &str {
    h.get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn mission(v: parvi_missions_contract::Mission) -> Value {
    serde_json::to_value(parvi_missions_contract::document::MissionDocument {
        mission: v,
        etag: None,
    })
    .expect("mission serializes")
}
fn tagged_mission(v: parvi_missions_contract::Mission, etag: &str) -> Value {
    serde_json::to_value(parvi_missions_contract::document::MissionDocument {
        mission: v,
        etag: Some(etag.into()),
    })
    .expect("mission serializes")
}
fn layout_value(v: parvi_missions_contract::Layout, etag: &str) -> Value {
    serde_json::to_value(parvi_missions_contract::document::LayoutDocument {
        layout: v,
        etag: etag.into(),
    })
    .expect("layout serializes")
}
async fn list(State(m): State<Missions>) -> ResultResponse {
    let v = m.store.list_missions()?;
    Ok(ok(json!({"missions":v})))
}
fn slug_from_title(title: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for c in title.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            slug.push(c);
            separator = false;
        } else if !slug.is_empty() && !separator {
            slug.push('-');
            separator = true;
        }
        if slug.len() >= 64 {
            break;
        }
    }
    slug.trim_matches('-').to_owned()
}
async fn create(State(m): State<Missions>, input: JsonBody<Value>) -> ResultResponse {
    let v = body(input, "bad_request")?;
    let title = s(&v, "title").trim();
    let given_slug = s(&v, "slug").trim();
    let unnamed = title.is_empty() && given_slug.is_empty();
    let title = if unnamed { "Untitled mission" } else { title };
    let slug = if given_slug.is_empty() {
        slug_from_title(title)
    } else {
        given_slug.to_owned()
    };
    if slug.is_empty() {
        return Err(bad("Mission name must contain a letter or number"));
    }
    let mut result = None;
    for suffix in 1..=100 {
        let name = if suffix == 1 {
            slug.clone()
        } else {
            format!("{slug}-{suffix}")
        };
        let display = if suffix == 1 {
            title.to_owned()
        } else {
            format!("{title} {suffix}")
        };
        match m.store.create_mission(&name, &display, s(&v, "updatedBy")) {
            Ok(edit) => {
                result = Some(edit);
                break;
            }
            Err(e) if e.code == "already_exists" && unnamed && suffix < 100 => continue,
            Err(e) if e.code == "already_exists" => {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "mission_exists".into(),
                    e.message,
                ));
            }
            Err(e) => return Err(e.into()),
        }
    }
    let result = result.ok_or_else(|| bad("no untitled mission name available"))?;
    m.changed();
    let mut r = tagged(
        json!({"mission":tagged_mission(result.mission,&result.etag)}),
        &result.etag,
    );
    *r.status_mut() = StatusCode::CREATED;
    Ok(r)
}
async fn read(State(m): State<Missions>, Path(id): Path<String>) -> ResultResponse {
    let e = m.store.mission_edit(&id)?;
    Ok(tagged(
        json!({"mission":tagged_mission(e.mission,&e.etag)}),
        &e.etag,
    ))
}
async fn edit(
    State(m): State<Missions>,
    Path(id): Path<String>,
    h: HeaderMap,
    input: Result<Bytes, BytesRejection>,
) -> ResultResponse {
    let input = input.map_err(|e| bad(e.body_text()))?;
    let patch::UniquePatch(v) = serde_json::from_slice(&input).map_err(|e| bad(format!("{e}")))?;
    let patch = patch::decode(&v).map_err(bad)?;
    let nested = v.as_object().and_then(|body| {
        body.iter()
            .find(|(key, _)| key.as_str() != "expectedRev" && key.as_str() != "updatedBy")
            .map(|(_, value)| value)
    });
    let p = Precondition {
        rev: v["expectedRev"]
            .as_i64()
            .or_else(|| nested.and_then(|v| v["expectedRev"].as_i64()))
            .unwrap_or_default(),
        etag: etag(&h).into(),
    };
    let before = m.store.mission_edit(&id)?;
    let actor = if s(&v, "updatedBy").is_empty() {
        nested.map(|n| s(n, "updatedBy")).unwrap_or("")
    } else {
        s(&v, "updatedBy")
    };
    let e = m.store.patch(&id, p, patch, actor)?;
    if e.etag != before.etag {
        m.changed();
    }
    let layout = m.store.layout_edit(&id)?;
    let prior = mission(before.mission);
    let current = tagged_mission(e.mission, &e.etag);
    let mut body = json!({"mission":current,"layout":layout_value(layout.layout,&layout.etag)});
    for (op, list, key) in [
        ("createInputCard", "inputCards", "inputCard"),
        ("createFormation", "formations", "formation"),
        ("createGate", "gates", "gate"),
        ("createEnd", "ends", "end"),
        ("createLimit", "limits", "limit"),
    ] {
        if v.get(op).is_some() {
            let old = prior[list].as_array();
            if let Some(node) = body["mission"][list]
                .as_array()
                .and_then(|nodes| {
                    nodes.iter().find(|node| {
                        !old.is_some_and(|nodes| nodes.iter().any(|old| old["id"] == node["id"]))
                    })
                })
                .cloned()
            {
                body[key] = node;
            }
        }
    }
    for (op, key) in [
        ("deleteFormation", "formationId"),
        ("deleteGate", "gateId"),
        ("deleteEnd", "endId"),
        ("deleteLimit", "limitId"),
        ("deleteInputCard", "inputCardId"),
    ] {
        if let Some(id) = v[op]["id"].as_str() {
            body[key] = json!(id);
        }
    }
    Ok(tagged(body, &e.etag))
}
async fn delete(
    State(m): State<Missions>,
    Path(id): Path<String>,
    h: HeaderMap,
    input: JsonBody<Value>,
) -> ResultResponse {
    let v = body(input, "bad_request")?;
    let e = m.store.delete_mission(
        &id,
        Precondition {
            rev: v["expectedRev"].as_i64().unwrap_or_default(),
            etag: etag(&h).into(),
        },
    )?;
    m.changed();
    Ok(ok(
        json!({"deletion":{"id":e.id,"slug":e.slug,"title":e.title,"archiveId":e.archive_id}}),
    ))
}
async fn validation(State(m): State<Missions>, Path(id): Path<String>) -> ResultResponse {
    let e = m.store.mission_edit(&id)?;
    let report = validate(&e.mission, &m.store.roles(), &Harnesses::from_host());
    Ok(tagged(
        json!({"missionRev":e.mission.rev,"missionEtag":e.etag,"errors":report.errors,"warnings":report.warnings}),
        &e.etag,
    ))
}
fn notes_value(notes: parvi_missions_contract::Notes, etag: &str) -> Value {
    serde_json::to_value(
        parvi_missions_contract::document::NotesDocument::from_notes(notes, etag.into()),
    )
    .expect("notes serialize")
}
async fn notes(State(m): State<Missions>, Path(id): Path<String>) -> ResultResponse {
    let e = m.store.notes_edit(&id)?;
    Ok(tagged(
        json!({"notes":notes_value(e.notes,&e.etag)}),
        &e.etag,
    ))
}
async fn edit_notes(
    State(m): State<Missions>,
    Path(id): Path<String>,
    h: HeaderMap,
    input: JsonBody<Value>,
) -> ResultResponse {
    let v = body(input, "bad_request")?;
    let target = s(&v, "target").into();
    let author = if s(&v, "author").is_empty() {
        "human:ui".into()
    } else {
        s(&v, "author").into()
    };
    let text = s(&v, "text").into();
    let id_entry = s(&v, "entryId").into();
    let p = match s(&v, "action") {
        "" | "append" => NotePatch::Append {
            target,
            author,
            text,
        },
        "edit" => NotePatch::Edit {
            target,
            author,
            id: id_entry,
            text,
        },
        "delete" => NotePatch::Delete {
            target,
            author,
            id: id_entry,
        },
        _ => return Err(bad("unknown note action")),
    };
    let e = m.store.patch_notes(&id, etag(&h), p)?;
    m.changed();
    Ok(tagged(
        json!({"notes":notes_value(e.notes,&e.etag)}),
        &e.etag,
    ))
}
async fn layout(State(m): State<Missions>, Path(id): Path<String>) -> ResultResponse {
    let e = m.store.layout_edit(&id)?;
    Ok(tagged(
        json!({"layout":layout_value(e.layout,&e.etag)}),
        &e.etag,
    ))
}
async fn edit_layout(
    State(m): State<Missions>,
    Path(id): Path<String>,
    h: HeaderMap,
    input: JsonBody<Value>,
) -> ResultResponse {
    let v = body(input, "bad_request")?;
    let p = if v["arrange"].as_bool() == Some(true) {
        let e = m.store.mission_edit(&id)?;
        let l = m.store.layout_edit(&id)?;
        LayoutPatch::Nodes(parvi_mission_engine::arrange(&e.mission, &l.layout).nodes)
    } else if let Some(nodes) = v.get("nodes") {
        LayoutPatch::Nodes(serde_json::from_value(nodes.clone()).map_err(|e| bad(e.to_string()))?)
    } else if let Some(edges) = v.get("edges") {
        LayoutPatch::Edges(serde_json::from_value(edges.clone()).map_err(|e| bad(e.to_string()))?)
    } else {
        return Err(bad("layout needs nodes, edges or arrange"));
    };
    let e = m.store.patch_layout(&id, etag(&h), p)?;
    m.changed();
    Ok(tagged(
        json!({"layout":layout_value(e.layout,&e.etag)}),
        &e.etag,
    ))
}

fn character_store(m: &Missions) -> parvi_mission_engine::CharacterStore {
    parvi_mission_engine::CharacterStore::new(m.store.roles().dir())
}
fn engine_character(c: parvi_missions_contract::Character) -> parvi_mission_engine::Character {
    parvi_mission_engine::Character {
        id: c.id,
        name: c.name,
        description: c.description,
        picture: c.picture,
        agent_id: c.agent_id,
        harness: c.harness,
        model: c.model,
        effort: c.effort,
    }
}
fn wire_character(c: parvi_mission_engine::Character) -> parvi_missions_contract::Character {
    parvi_missions_contract::Character {
        id: c.id,
        name: c.name,
        description: c.description,
        picture: c.picture,
        agent_id: c.agent_id,
        harness: c.harness,
        model: c.model,
        effort: c.effort,
    }
}
async fn characters(State(m): State<Missions>) -> ResultResponse {
    let characters = character_store(&m)
        .list()?
        .into_iter()
        .map(wire_character)
        .collect();
    Ok(Json(parvi_missions_contract::Characters { characters }).into_response())
}
async fn create_character(
    State(m): State<Missions>,
    input: JsonBody<parvi_missions_contract::Character>,
) -> ResultResponse {
    let c = engine_character(body(input, "invalid_character")?);
    let c = wire_character(character_store(&m).create(c)?);
    m.changed();
    Ok((StatusCode::CREATED, Json(c)).into_response())
}
async fn put_character(
    State(m): State<Missions>,
    Path(id): Path<String>,
    input: JsonBody<parvi_missions_contract::Character>,
) -> ResultResponse {
    let mut c = body(input, "invalid_character")?;
    if !c.id.is_empty() && c.id != id {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_character".into(),
            "character id does not match the path".into(),
        ));
    }
    c.id = id;
    let c = wire_character(character_store(&m).put(engine_character(c))?);
    m.changed();
    Ok(Json(c).into_response())
}
async fn delete_character(State(m): State<Missions>, Path(id): Path<String>) -> ResultResponse {
    character_store(&m).delete(&id)?;
    m.changed();
    Ok(Json(parvi_missions_contract::Deleted { deleted: id }).into_response())
}
async fn gate_profiles() -> Response {
    let profiles: parvi_missions_contract::GateProfiles =
        serde_json::from_str(include_str!("gate_profiles.json")).expect("gate profiles are valid");
    Json(profiles).into_response()
}
