//! Role cards share the mission engine's storage and reference locks.
use axum::{
    Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use parvi_agents_contract::roles::{Note, PATH, Role, Roles, Unreadable, Usage};
use parvi_core::Error;
use parvi_mission_engine::{RoleCard, RoleEdit, RoleStore};
use parvi_node_kit::{Command, args::object};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

pub struct Service {
    store: RoleStore,
    codex_home: Option<PathBuf>,
}
impl Service {
    pub fn new(root: PathBuf, codex_home: Option<PathBuf>) -> Self {
        Self {
            store: parvi_mission_engine::Store::new(root).roles(),
            codex_home,
        }
    }
    fn role(card: RoleCard, etag: String) -> Role {
        Role {
            schema: 1,
            id: card.id,
            display_name: card.display_name,
            kind: card.kind,
            summary: card.summary,
            tags: card.tags,
            status: card.status,
            etag,
            preset: card.builtin,
            notes: card
                .notes
                .into_iter()
                .map(|n| Note {
                    ts: n.ts,
                    actor: n.actor,
                    text: n.text,
                })
                .collect(),
        }
    }
    fn edited(e: RoleEdit) -> Role {
        Self::role(e.card, e.etag)
    }
    fn list(&self, query: &Filter) -> Result<Roles, Error> {
        let (mut cards, unreadable) = self.store.list()?;
        if let Some(capable) = query.capable.as_deref().filter(|s| !s.is_empty()) {
            cards.retain(|c| c.tags.iter().any(|t| t == capable));
        }
        if query.assignable {
            cards.retain(|c| c.status != "retired");
        }
        let harnesses = self.codex_home.as_deref().map_or_else(
            parvi_mission_engine::Harnesses::from_host,
            parvi_mission_engine::Harnesses::from_codex_home,
        );
        let harnesses = [
            (
                "claude-code",
                &["low", "medium", "high", "xhigh", "max"][..],
            ),
            (
                "openai-codex",
                &["low", "medium", "high", "xhigh", "max", "ultra"][..],
            ),
            ("pi", &["low", "medium", "high", "xhigh", "max"][..]),
        ]
        .into_iter()
        .map(|(id, efforts)| parvi_agents_contract::roles::Harness {
            id: id.into(),
            efforts: efforts.iter().map(|s| (*s).into()).collect(),
            models: harnesses.models(id),
        })
        .collect();
        Ok(Roles {
            roles: cards
                .into_iter()
                .map(|c| Self::role(c, String::new()))
                .collect(),
            unreadable: unreadable
                .into_iter()
                .map(|u| Unreadable {
                    name: u.name,
                    reason: u.reason,
                })
                .collect(),
            harnesses,
            effort_policy: [
                ("low", "errands"),
                ("medium", "making things"),
                ("xhigh", "architecture and review"),
                ("max", "consequential reviews"),
            ]
            .into_iter()
            .map(
                |(effort, usage)| parvi_agents_contract::roles::EffortPolicy {
                    effort: effort.into(),
                    usage: usage.into(),
                },
            )
            .collect(),
        })
    }
    fn read(&self, id: &str) -> Result<Role, Error> {
        Ok(Self::edited(self.store.role_edit(id)?))
    }
    fn usage(&self, id: &str) -> Result<Usage, Error> {
        self.store.role_edit(id)?;
        serde_json::from_value(serde_json::to_value(self.store.usage(id)?).map_err(bad)?)
            .map_err(bad)
    }
    fn create(&self, v: &Value) -> Result<Role, Error> {
        fields(
            v,
            &[
                "id",
                "displayName",
                "kind",
                "summary",
                "capabilities",
                "personality",
            ],
        )?;
        let mut card = RoleCard {
            id: text(v, "id").into(),
            display_name: text(v, "displayName").into(),
            kind: text(v, "kind").into(),
            summary: text(v, "summary").into(),
            tags: serde_json::from_value(v.get("capabilities").cloned().unwrap_or(json!([])))
                .map_err(bad)?,
            ..RoleCard::default()
        };
        if !text(v, "personality").is_empty() {
            card.tags
                .push(format!("personality:{}", text(v, "personality")));
        }
        Ok(Self::edited(self.store.create(card)?))
    }
    fn edit(&self, id: &str, etag: &str, v: &Value) -> Result<Role, Error> {
        fields(
            v,
            &[
                "displayName",
                "kind",
                "summary",
                "capabilities",
                "addCapability",
                "removeCapability",
                "note",
                "updatedBy",
                "retire",
            ],
        )?;
        if let Some(retire) = v["retire"].as_bool() {
            return Ok(Self::edited(if retire {
                self.store.retire(id, etag)?
            } else {
                self.store.restore(id, etag)?
            }));
        }
        let mut card = self.store.role_edit(id)?.card;
        for (key, field) in [
            ("displayName", &mut card.display_name),
            ("kind", &mut card.kind),
            ("summary", &mut card.summary),
        ] {
            if let Some(value) = v[key].as_str() {
                *field = value.into();
            }
        }
        if let Some(tags) = v.get("capabilities") {
            let requested: Vec<String> = serde_json::from_value(tags.clone()).map_err(bad)?;
            card.tags.retain(|t| t.contains(':'));
            for t in requested {
                let t = t.trim();
                if !t.is_empty() && !card.tags.iter().any(|v| v == t) {
                    card.tags.push(t.into());
                }
            }
        }
        if let Some(tag) = v["addCapability"].as_str()
            && !card.tags.iter().any(|t| t == tag)
        {
            card.tags.push(tag.into());
        }
        if let Some(tag) = v["removeCapability"].as_str() {
            card.tags.retain(|t| t != tag);
        }
        let edit = if let Some(note) = v["note"].as_str().filter(|s| !s.is_empty()) {
            self.store
                .edit_with_note(id, etag, card, text(v, "updatedBy"), note)?
        } else {
            self.store.edit(id, etag, card)?
        };
        Ok(Self::edited(edit))
    }
    fn delete(&self, id: &str, etag: &str) -> Result<Value, Error> {
        let builtin = self.store.role_edit(id)?.card.builtin;
        self.store.delete(id, etag)?;
        Ok(json!({"deleted":id,"builtinRemains":builtin}))
    }
}
fn bad(e: impl std::fmt::Display) -> Error {
    Error::new("invalid_agent_card", e.to_string())
}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn fields(v: &Value, allowed: &[&str]) -> Result<(), Error> {
    let fields = v.as_object().ok_or_else(|| bad("expected a role object"))?;
    if let Some(key) = fields.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(bad(format!("role request field {key:?} is not supported")));
    }
    Ok(())
}
#[derive(Default)]
struct Filter {
    capable: Option<String>,
    assignable: bool,
}
fn reply<T: serde::Serialize>(result: Result<T, Error>) -> Response {
    match result {
        Ok(v) => axum::Json(json!(v)).into_response(),
        Err(e) => {
            let status = match e.code.as_str() {
                "not_found" => StatusCode::NOT_FOUND,
                "precondition_required" => StatusCode::PRECONDITION_REQUIRED,
                "conflict" | "already_exists" | "role_in_use" => StatusCode::CONFLICT,
                _ => StatusCode::BAD_REQUEST,
            };
            (status, axum::Json(json!({"error":e}))).into_response()
        }
    }
}
fn reply_role(result: Result<Role, Error>) -> Response {
    match result {
        Ok(role) => {
            let mut response = axum::Json(json!(role)).into_response();
            if let Ok(value) = role.etag.parse() {
                response.headers_mut().insert(header::ETAG, value);
            }
            response
        }
        Err(error) => reply::<Role>(Err(error)),
    }
}
fn body(bytes: &Bytes) -> Result<Value, Error> {
    serde_json::from_slice(bytes).map_err(bad)
}
fn tag(h: &HeaderMap) -> &str {
    h.get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}
pub fn routes(s: Arc<Service>) -> Router {
    Router::new()
        .route(PATH, get(list).post(create))
        .route("/api/roles/{id}", get(read).patch(edit).delete(delete))
        .route("/api/roles/{id}/usage", get(usage))
        .with_state(s)
}
async fn list(
    State(s): State<Arc<Service>>,
    query: Result<
        Query<std::collections::HashMap<String, String>>,
        axum::extract::rejection::QueryRejection,
    >,
) -> Response {
    match query {
        Ok(Query(q)) => reply(
            s.list(&Filter {
                capable: q.get("capable").cloned(),
                assignable: q
                    .get("assignable")
                    .is_some_and(|v| ["true", "1", "yes", "on"].contains(&v.as_str())),
            }),
        ),
        Err(error) => reply::<Roles>(Err(bad(error))),
    }
}
async fn read(State(s): State<Arc<Service>>, Path(id): Path<String>) -> Response {
    reply_role(s.read(&id))
}
async fn usage(State(s): State<Arc<Service>>, Path(id): Path<String>) -> Response {
    reply(s.usage(&id))
}
async fn create(State(s): State<Arc<Service>>, bytes: Bytes) -> Response {
    let result = body(&bytes).and_then(|v| s.create(&v));
    let created = result.is_ok();
    let mut response = reply_role(result);
    if created {
        *response.status_mut() = StatusCode::CREATED;
    }
    response
}
async fn edit(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    reply_role(body(&bytes).and_then(|v| s.edit(&id, tag(&headers), &v)))
}
async fn delete(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    reply(s.delete(&id, tag(&headers)))
}

pub fn commands(s: Arc<Service>, node: String) -> Vec<Command> {
    const NAMES: &[&str] = &[
        "role_list",
        "role_inspect",
        "role_usage",
        "role_new",
        "role_edit",
        "role_retire",
        "role_restore",
        "role_delete",
        "role_characters",
    ];
    NAMES.iter().map(|&name| {
        let (s, node) = (s.clone(), node.clone());
        Command::new(name, "Read or change the role library.", move || {
            let (props, required) = match name {
                "role_list" => (json!({"capable":{"type":"string"},"assignable":{"type":"boolean"}}), vec![]),
                "role_characters" => (json!({}), vec![]),
                "role_new" => (json!({"role":{"type":"string"},"display_name":{"type":"string"},"kind":{"type":"string"},"summary":{"type":"string"},"capabilities":{"type":"array","items":{"type":"string"}},"personality":{"type":"string"}}), vec!["role"]),
                "role_edit" => (json!({"role":{"type":"string"},"change":{"type":"object"},"etag":{"type":"string"}}), vec!["role", "change"]),
                _ => (json!({"role":{"type":"string"},"etag":{"type":"string"}}), vec!["role"]),
            }; object(props, &required)
        }, move |args| {
            let (s, node) = (s.clone(), node.clone());
            async move {
                let id = text(&args, "role");
                let version = || -> Result<String, Error> { match args["etag"].as_str() { Some(tag) => Ok(tag.into()), None => Ok(s.read(id)?.etag) } };
                let mut result = match name {
                    "role_list" => serde_json::to_value(s.list(&Filter { capable: args["capable"].as_str().map(str::to_owned), assignable: args["assignable"].as_bool().unwrap_or(false) })?).map_err(bad)?,
                    "role_inspect" => json!(s.read(id)?),
                    "role_usage" => json!(s.usage(id)?),
                    "role_new" => json!(s.create(&json!({"id":id,"displayName":text(&args,"display_name"),"kind":text(&args,"kind"),"summary":text(&args,"summary"),"capabilities":args.get("capabilities").cloned().unwrap_or(json!([])),"personality":text(&args,"personality")}))?),
                    "role_edit" => json!(s.edit(id, &version()?, &args["change"])?),
                    "role_retire" | "role_restore" => { let usage = s.usage(id)?; let mut role = json!(s.edit(id, &version()?, &json!({"retire":name == "role_retire"}))?); role["usage"] = json!(usage); role },
                    "role_delete" => s.delete(id, &version()?)?,
                    "role_characters" => json!({"characters": parvi_mission_engine::CharacterStore::new(s.store.dir()).list()?}),
                    _ => unreachable!(),
                };
                result["node"] = json!(node);
                Ok(result)
            }
        })
    }).collect()
}
