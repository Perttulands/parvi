//! Serving a node: its areas, built from the config, behind the trust check
//! and the refusal of requests for other nodes, with the node's own
//! `/api/health`, `/api/version`, `/api/config`, `/ws` and `/mcp`, and its own
//! commands ([`crate::own`]).
//! Areas are checked before anything is served: two that share a name, a
//! frame kind or a command refuse to start, naming both
//! ([`crate::listing::check`]), as does one whose commands are not those it
//! lists.

mod addressed;
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
use parvi_core::{Error, PROTOCOL};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub(crate) use addressed::elsewhere;
pub use hello::hello;
pub use trust::Trust;

use crate::config::Config;
use crate::events::{self, Log};
use crate::identity::Identity;
use crate::mcp::Mcp;
use crate::own::{self, Own};
use crate::view::Views;
use crate::{Area, Built, Command, Listing, Provided, Setup, address, listing};

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
    /// Each built area, with how it is listed.
    areas: Vec<(Listing, Arc<dyn Area>)>,
    /// View control's side of every connection.
    views: Arc<Views>,
}

/// A node serving until stopped.
pub struct Running {
    pub address: SocketAddr,
    pub node: Identity,
    /// What its areas provide one another.
    pub provided: Provided,
    shared: Arc<Shared>,
    events: Arc<Log>,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<std::io::Result<()>>,
    _recorded: address::Recorded,
}

/// Builds the areas `config` leaves on, in `areas`' order, checks them and
/// serves them.
pub async fn start(options: Options, areas: &[Listing]) -> Result<Running> {
    let Options {
        dir,
        config,
        listen,
    } = options;
    listing::check(areas)?;
    let names: Vec<&str> = areas.iter().map(|area| area.name).collect();
    config.known(&names)?;
    let section = config.node()?;
    let node = Identity::load(&dir, section.device.clone())?;
    let listen = match listen {
        Some(listen) => listen,
        None => section.address(node.environment)?,
    };
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
        provided: Provided::default(),
    };
    // Each area is built and fills what it provides, then those that read
    // what others provide finish: the list's order does not matter.
    let mut started = Vec::new();
    let mut off = Vec::new();
    for &listed in areas {
        if !setup.config.enabled(listed.name)? {
            off.push(listed.name);
            continue;
        }
        let name = listed.name;
        started.push((
            listed,
            (listed.build)(&setup).with_context(|| format!("starting {name}"))?,
        ));
    }
    setup.provided.seal();
    let mut built = Vec::new();
    for (listed, area) in started {
        let area = match area {
            Built::Now(area) => area,
            Built::Then(finish) => {
                finish(&setup.provided).with_context(|| format!("starting {}", listed.name))?
            }
        };
        built.push((listed, area));
    }
    let own = Arc::new(Own {
        config: setup.config.clone(),
        areas: names.clone(),
        node: node.id.clone(),
        views: Arc::default(),
    });
    let mut commands = commands(&built)?;
    commands.extend(own::commands(&own));
    let here = format!("{} · {}", node.device, node.environment.name());
    events.record(
        events::NODE,
        false,
        format!("Parvi {} started on {here}", crate::VERSION),
    );
    let shared = Arc::new(Shared {
        node: node.clone(),
        off,
        areas: built,
        views: own.views.clone(),
    });
    let trust = Arc::new(Trust::local(section.names));
    let mut app = Router::new()
        .route("/ws", get(upgrade))
        .route("/api/health", get(health))
        .route("/api/version", get(version))
        .with_state(shared.clone())
        .merge(own::routes(own))
        .merge(Mcp::new(node.id.clone(), commands).routes());
    for (_, area) in &shared.areas {
        app = app.merge(area.routes());
    }
    let app = app
        .layer(middleware::from_fn_with_state(
            node.id.clone(),
            addressed::guard,
        ))
        .layer(middleware::from_fn_with_state(trust, trust::guard));
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
        provided: setup.provided.clone(),
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
        for (_, area) in &self.shared.areas {
            area.stop().await;
        }
        Ok(())
    }

    /// The areas that were not built because their section turned them off.
    pub fn off(&self) -> &[&'static str] {
        &self.shared.off
    }
}

/// Every built area's commands, refusing an area whose commands are not
/// those it lists.
fn commands(areas: &[(Listing, Arc<dyn Area>)]) -> Result<Vec<Command>, Error> {
    let mut all = Vec::new();
    for (listed, area) in areas {
        let commands = area.commands();
        let mut have: Vec<&str> = commands.iter().map(|c| c.name).collect();
        let mut listed_names = listed.commands.to_vec();
        have.sort();
        listed_names.sort();
        if have != listed_names {
            return Err(Error::new(
                "commands_unlisted",
                format!(
                    "area {} lists the commands {} but has {}",
                    listed.name,
                    listed_names.join(", "),
                    have.join(", ")
                ),
            ));
        }
        all.extend(commands);
    }
    Ok(all)
}

/// `GET /api/health`: this node answers, saying what it is and which areas
/// it turned off. Installers and clients read it.
async fn health(State(shared): State<Arc<Shared>>) -> Response {
    axum::Json(serde_json::json!({
        "status": "ok",
        "version": crate::VERSION,
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
    axum::Json(serde_json::json!({ "version": crate::VERSION })).into_response()
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
