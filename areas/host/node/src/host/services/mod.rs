//! Services: the host's catalog of services with their state, started and
//! stopped through systemd (#188). Parvi owns this itself, with CHROTE's
//! catalog, group and partial-state rules; it reads only when asked, so it
//! costs nothing while nobody looks.
//!
//! `GET /api/services` lists `{services:[{id,name,description,link,state,
//! controllable,components}]}`, and `GET /api/services/{id}` reads one
//! service's units alone (a client following its own action).
//! `POST /api/services/{id}/{start|stop|restart}` answers `{service,
//! errors, outcome}`: 200 `done` when every unit did as told, 502 `failed`
//! with systemd's words, and 202 `unconfirmed` when a unit gave no answer
//! in time, because the job systemd accepted may still finish and only a
//! later read can say. Starting goes through the components in order,
//! stopping in reverse, restarting is the stop then the start, all within
//! [`ACTION`]; nothing is ever enabled or disabled.

mod catalog;
mod systemd;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use parvi_host_contract::{Outcome, Service};
use tokio::sync::Mutex;

use catalog::Entry;
use systemd::Refusal;

/// The most one action may take, every unit of it: clients wait this long
/// for an answer, and a little more.
pub const ACTION: Duration = Duration::from_secs(40);

/// The node's services: where its catalog is, and one action at a time.
pub struct Services {
    catalog: PathBuf,
    events: std::sync::Arc<parvi_node_kit::events::Log>,
    acting: Mutex<()>,
}

/// A service's state from its components' (CHROTE's controller, in order).
pub fn group(components: &[&str]) -> &'static str {
    let all = |s: &str| components.iter().all(|c| *c == s);
    let any = |s: &[&str]| components.iter().any(|c| s.contains(c));
    if all("active") {
        "running"
    } else if all("inactive") {
        "stopped"
    } else if any(&["failed"]) {
        "failed"
    } else if any(&["unknown"]) {
        "unknown"
    } else if any(&["activating", "reloading"]) {
        "starting"
    } else if any(&["deactivating"]) {
        "stopping"
    } else {
        "partial"
    }
}

async fn status(e: &Entry) -> Service {
    let reads = e.components.iter().map(systemd::state);
    let components = futures_util::future::join_all(reads).await;
    Service {
        id: e.id.clone(),
        name: e.name.clone(),
        description: e.description.clone(),
        link: e.link.clone(),
        state: group(&components).to_string(),
        controllable: !e.read_only,
        components: components.into_iter().map(str::to_owned).collect(),
    }
}

impl Services {
    pub fn with_events(mut self, events: std::sync::Arc<parvi_node_kit::events::Log>) -> Self {
        self.events = events;
        self
    }

    pub fn new(catalog: PathBuf) -> Services {
        Services {
            catalog,
            events: Default::default(),
            acting: Mutex::new(()),
        }
    }

    fn entries(&self) -> Result<Vec<Entry>, Refused> {
        if !self.catalog.exists() {
            return Err(Refused(
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "No service catalog on this node: write one at {}",
                    self.catalog.display()
                ),
            ));
        }
        catalog::read(&self.catalog).map_err(|e| Refused(StatusCode::SERVICE_UNAVAILABLE, e))
    }

    pub async fn list(&self) -> Result<Vec<Service>, Refused> {
        let entries = self.entries()?;
        Ok(futures_util::future::join_all(entries.iter().map(status)).await)
    }

    fn entry(&self, id: &str) -> Result<Entry, Refused> {
        self.entries()?
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| {
                Refused(
                    StatusCode::NOT_FOUND,
                    format!("No service {id} in the catalog"),
                )
            })
    }

    /// One service as its units are now.
    pub async fn one(&self, id: &str) -> Result<Service, Refused> {
        Ok(status(&self.entry(id)?).await)
    }

    /// Starts, stops or restarts every unit of a service within
    /// [`ACTION`]; each failure in systemd's words. Every unit of a start
    /// or a stop is tried, whatever the one before did; a restart starts
    /// only once every unit has stopped.
    pub async fn act(
        &self,
        id: &str,
        action: &str,
    ) -> Result<(Service, Vec<String>, Outcome), Refused> {
        let bad = |m: &str| Refused(StatusCode::BAD_REQUEST, m.to_string());
        let steps: &[&str] = match action {
            "start" => &["start"],
            "stop" => &["stop"],
            "restart" => &["stop", "start"],
            _ => return Err(bad("A service starts, stops or restarts.")),
        };
        let entry = self.entry(id)?;
        if entry.read_only {
            return Err(bad(
                "This service is read-only: the host shows it but does not let Parvi change it.",
            ));
        }
        let _one = self.acting.lock().await;
        let deadline = tokio::time::Instant::now() + ACTION;
        let mut errors = vec![];
        let mut outcome = Outcome::Done;
        for (n, verb) in steps.iter().enumerate() {
            if n > 0 && outcome != Outcome::Done {
                errors.push(format!("Not {verb}ed: the stop did not finish."));
                break;
            }
            let mut order: Vec<_> = entry.components.iter().collect();
            if *verb == "stop" {
                order.reverse();
            }
            for c in order {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    errors.push(format!("Not tried: the {action} ran out of time."));
                    outcome = max_outcome(outcome, Outcome::Failed);
                    continue;
                }
                match systemd::act(c, verb, left.min(systemd::SYSTEMCTL)).await {
                    Ok(()) => {}
                    Err(Refusal::Failed(e)) => {
                        errors.push(e);
                        outcome = max_outcome(outcome, Outcome::Failed);
                    }
                    Err(Refusal::Unconfirmed(e)) => {
                        errors.push(e);
                        outcome = Outcome::Unconfirmed;
                    }
                }
            }
        }
        let name = &entry.name;
        let (failure, text) = match outcome {
            Outcome::Done => (false, format!("{name}: {action} done")),
            Outcome::Failed => (
                true,
                format!("{name}: {action} failed: {}", errors.join("; ")),
            ),
            Outcome::Unconfirmed => (
                true,
                format!("{name}: {action} unconfirmed: {}", errors.join("; ")),
            ),
        };
        self.events.record("service", failure, text);
        Ok((status(&entry).await, errors, outcome))
    }
}

/// Unconfirmed outweighs failed: something may still be moving.
fn max_outcome(a: Outcome, b: Outcome) -> Outcome {
    match (a, b) {
        (Outcome::Unconfirmed, _) | (_, Outcome::Unconfirmed) => Outcome::Unconfirmed,
        (Outcome::Failed, _) | (_, Outcome::Failed) => Outcome::Failed,
        _ => Outcome::Done,
    }
}

/// A refusal: its status and its sentence, answered as `{error}`.
#[derive(Debug)]
pub struct Refused(StatusCode, String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}

impl Refused {
    pub fn error(&self) -> parvi_core::Error {
        let code = match self.0 {
            StatusCode::BAD_REQUEST => parvi_host_contract::BAD_REQUEST,
            StatusCode::NOT_FOUND => parvi_host_contract::NOT_FOUND,
            _ => parvi_host_contract::UNAVAILABLE,
        };
        parvi_core::Error::new(code, &self.1)
    }
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        let error = self.error();
        (self.0, Json(serde_json::json!({ "error": error }))).into_response()
    }
}

/// The routes, for the server to merge with the other modules'.
pub fn routes<S: Clone + Send + Sync + 'static>(services: Arc<Services>) -> Router<S> {
    Router::new()
        .route("/api/services", get(list))
        .route("/api/services/{id}", get(one))
        .route("/api/services/{id}/{action}", post(act))
        .with_state(services)
}

async fn list(State(s): State<Arc<Services>>) -> Response {
    match s.list().await {
        Ok(services) => Json(parvi_host_contract::ServiceList { services }).into_response(),
        Err(r) => r.into_response(),
    }
}

async fn one(State(s): State<Arc<Services>>, Path(id): Path<String>) -> Response {
    match s.one(&id).await {
        Ok(service) => Json(parvi_host_contract::ServiceOne { service }).into_response(),
        Err(r) => r.into_response(),
    }
}

async fn act(
    State(s): State<Arc<Services>>,
    Path((id, action)): Path<(String, String)>,
) -> Response {
    match s.act(&id, &action).await {
        Ok((service, errors, outcome)) => {
            let code = match outcome {
                Outcome::Done => StatusCode::OK,
                Outcome::Failed => StatusCode::BAD_GATEWAY,
                Outcome::Unconfirmed => StatusCode::ACCEPTED,
            };
            let body = parvi_host_contract::ServiceAction {
                service,
                errors,
                outcome,
            };
            (code, Json(body)).into_response()
        }
        Err(r) => r.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_reads_as_chrotes_controller_does() {
        assert_eq!(group(&["active", "active"]), "running");
        assert_eq!(group(&["inactive", "inactive"]), "stopped");
        assert_eq!(group(&["active", "inactive"]), "partial");
        assert_eq!(group(&["active", "failed"]), "failed");
        assert_eq!(group(&["active", "unknown"]), "unknown");
        assert_eq!(group(&["active", "activating"]), "starting");
        assert_eq!(group(&["inactive", "deactivating"]), "stopping");
        assert_eq!(
            group(&["failed", "unknown"]),
            "failed",
            "failed comes first"
        );
    }

    #[tokio::test]
    async fn refuses_an_unknown_action_an_unknown_id_and_a_read_only_service() {
        let dir = std::env::temp_dir().join(format!("parvi-services-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("services.json");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let responder = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
        });
        std::fs::write(
            &file,
            format!(
                r#"{{"services":[{{"id":"room","name":"Room","readOnly":true,
                "components":[{{"kind":"http","url":"http://{address}/"}}]}}]}}"#
            ),
        )
        .unwrap();
        let s = Services::new(file);
        let code = |r: Result<_, Refused>| r.err().map(|r| r.0);
        assert_eq!(
            code(s.act("room", "reload").await),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            code(s.act("nope", "start").await),
            Some(StatusCode::NOT_FOUND)
        );
        assert_eq!(
            code(s.act("room", "start").await),
            Some(StatusCode::BAD_REQUEST)
        );
        let listed = s.list().await.unwrap();
        assert_eq!(listed[0].state, "running");
        responder.await.unwrap();
        assert!(!listed[0].controllable);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn without_a_catalog_the_node_says_where_to_write_one() {
        let s = Services::new(PathBuf::from("/nonexistent/parvi/services.json"));
        let r = s.list().await.unwrap_err();
        assert_eq!(r.0, StatusCode::SERVICE_UNAVAILABLE);
        assert!(r.1.contains("/nonexistent/parvi/services.json"));
    }
}
