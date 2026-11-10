//! Serving a node: its areas, built from the config, behind the trust check,
//! with the node's own `/api/health`, `/api/version`, `/ws` and `/mcp`.
//! Areas are checked before anything is served: two that share a name, a
//! frame kind or a command refuse to start, naming both.

mod authority;
mod hello;
mod socket;
mod trust;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::extract::ws::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use parvi_core::frame::{self, HELLO};
use parvi_core::{Error, PROTOCOL};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub use hello::hello;
pub use trust::Trust;

use crate::config::Config;
use crate::events::{self, Log};
use crate::identity::{Identity, Ports};
use crate::mcp::Mcp;
use crate::{Area, Build, Command, Setup, address};

/// How a node is started.
pub struct Options {
    /// The node's config folder: its id and recorded address live there.
    pub dir: PathBuf,
    pub config: Config,
    /// Overrides `[node] listen`.
    pub listen: Option<String>,
}

/// What every connection and request shares.
struct Shared {
    node: Identity,
    off: Vec<&'static str>,
    areas: Vec<Arc<dyn Area>>,
}

/// A node serving until stopped.
pub struct Running {
    pub address: SocketAddr,
    pub node: Identity,
    /// The node's sessions, if an area provides them.
    pub sessions: crate::Provided<dyn crate::Sessions>,
    shared: Arc<Shared>,
    events: Arc<Log>,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<std::io::Result<()>>,
    _recorded: address::Recorded,
}

/// Builds the areas `config` leaves on, in `areas`' order, checks them and
/// serves them.
pub async fn start(options: Options, areas: &[(&'static str, Build)]) -> Result<Running> {
    let Options {
        dir,
        config,
        listen,
    } = options;
    let names: Vec<&str> = areas.iter().map(|(name, _)| *name).collect();
    config.known(&names)?;
    let section = config.node()?;
    let node = Identity::load(&dir, section.device.clone())?;
    let mut ports = Ports::default();
    for (environment, port) in &section.ports {
        ports.set(&format!("{environment}={port}"))?;
    }
    let listen = listen
        .or(section.listen.clone())
        .unwrap_or_else(|| format!("127.0.0.1:{}", ports.of(node.environment)));
    let listener = TcpListener::bind(&listen)
        .await
        .with_context(|| format!("listening on {listen}"))?;
    let address = listener.local_addr()?;
    let events = Arc::new(Log::default());
    let setup = Setup {
        node: node.clone(),
        dir: dir.clone(),
        home: std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from),
        config,
        address,
        events: events.clone(),
        sessions: crate::Provided::new("sessions"),
    };
    let mut built: Vec<Arc<dyn Area>> = Vec::new();
    let mut off = Vec::new();
    for &(name, build) in areas {
        if !setup.config.enabled(name)? {
            off.push(name);
            continue;
        }
        let area = build(&setup).map_err(|error| unbuilt(name, error))?;
        if area.name() != name {
            return Err(Error::new(
                "area_conflict",
                format!("the area listed as {name} calls itself {}", area.name()),
            )
            .into());
        }
        built.push(area);
    }
    let commands = check(&built)?;
    let here = format!("{} · {}", node.device, node.environment.name());
    events.record(
        events::NODE,
        false,
        format!("Parvi {} started on {here}", env!("CARGO_PKG_VERSION")),
    );
    let shared = Arc::new(Shared {
        node: node.clone(),
        off,
        areas: built,
    });
    let trust = Arc::new(Trust::local(section.names));
    let mut app = Router::new()
        .route("/ws", get(upgrade))
        .route("/api/health", get(health))
        .route("/api/version", get(version))
        .with_state(shared.clone())
        .merge(Mcp::new(node.id.clone(), commands).routes());
    for area in &shared.areas {
        app = app.merge(area.routes());
    }
    let app = app.layer(middleware::from_fn_with_state(trust, trust::guard));
    let recorded = address::record(&dir, address)?;
    eprintln!(
        "parvi: node {} ({}, {}) serving http://{address}",
        node.id,
        node.device,
        node.environment.name(),
    );
    let (stop, stopped) = oneshot::channel::<()>();
    let serving = tokio::spawn(
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .into_future(),
    );
    Ok(Running {
        address,
        node,
        sessions: setup.sessions.clone(),
        shared,
        events,
        stop,
        serving,
        _recorded: recorded,
    })
}

impl Running {
    /// Stops serving, then stops each area. Clients still connected are
    /// cut off; an area's channel gives back what the client held.
    pub async fn stop(self) -> Result<()> {
        let here = format!("{} · {}", self.node.device, self.node.environment.name());
        self.events
            .record(events::NODE, false, format!("Parvi stopping on {here}"));
        let _ = self.stop.send(());
        // Open sockets keep a graceful shutdown waiting; they are dropped
        // with the task.
        let served = tokio::time::timeout(std::time::Duration::from_secs(2), self.serving).await;
        if let Ok(served) = served {
            served.context("serving")?.context("serving")?;
        }
        for area in &self.shared.areas {
            area.stop().await;
        }
        Ok(())
    }

    /// The areas that were not built because their section turned them off.
    pub fn off(&self) -> &[&'static str] {
        &self.shared.off
    }
}

/// Refuses areas that share a name, a frame kind or a command, naming both,
/// and commands not named `noun_verb`; answers with every command.
fn check(areas: &[Arc<dyn Area>]) -> Result<Vec<Command>, Error> {
    let conflict = |message: String| Error::new("area_conflict", message);
    let mut commands: Vec<(&str, Command)> = Vec::new();
    for (i, area) in areas.iter().enumerate() {
        if frame::owns(area.kinds(), HELLO) {
            return Err(conflict(format!(
                "area {} owns frame kind {HELLO:#04x}, the node's hello",
                area.name()
            )));
        }
        for other in &areas[..i] {
            if other.name() == area.name() {
                return Err(conflict(format!("two areas are named {}", area.name())));
            }
            if frame::overlap(other.kinds(), area.kinds()) {
                return Err(conflict(format!(
                    "areas {} and {} own the same frame kinds",
                    other.name(),
                    area.name()
                )));
            }
        }
        for command in area.commands() {
            if !command.well_named() {
                return Err(conflict(format!(
                    "area {}: command {} is not named noun_verb",
                    area.name(),
                    command.name
                )));
            }
            if let Some((owner, _)) = commands.iter().find(|(_, c)| c.name == command.name) {
                return Err(conflict(format!(
                    "areas {owner} and {} both have the command {}",
                    area.name(),
                    command.name
                )));
            }
            commands.push((area.name(), command));
        }
    }
    Ok(commands.into_iter().map(|(_, command)| command).collect())
}

/// `GET /api/health`: this node answers, saying what it is and which areas
/// it turned off. Installers and clients read it.
async fn health(State(shared): State<Arc<Shared>>) -> Response {
    axum::Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL,
        "node": shared.node.id,
        "device": shared.node.device,
        "environment": shared.node.environment.name(),
        "off": shared.off,
        "timestamp": jiff::Timestamp::now().strftime("%Y-%m-%dT%H:%M:%SZ").to_string(),
    }))
    .into_response()
}

/// `GET /api/version`, as CHROTE's.
async fn version() -> Response {
    axum::Json(serde_json::json!({ "version": env!("CARGO_PKG_VERSION") })).into_response()
}

async fn upgrade(ws: WebSocketUpgrade, State(shared): State<Arc<Shared>>) -> Response {
    ws.on_upgrade(move |socket| socket::connection(socket, shared))
}

/// An HTTP route's failure: its status, and the error as
/// `{"error":{"code","message"}}`.
pub struct Failure(pub StatusCode, pub Error);

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.1 });
        (self.0, axum::Json(body)).into_response()
    }
}

/// Why `name` could not be built. An area that reads a slot another area
/// fills fails with `not_provided` when the filling area comes later in the
/// list, so that error says what to change.
fn unbuilt(name: &str, error: anyhow::Error) -> anyhow::Error {
    let early = error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<Error>())
        .any(|cause| cause.code == "not_provided");
    let error = error.context(format!("starting {name}"));
    if early {
        error.context(format!(
            "{name} reads what another area provides, which must be built before it: \
             list that area earlier, or turn it on"
        ))
    } else {
        error
    }
}
