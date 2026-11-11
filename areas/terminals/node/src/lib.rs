//! The terminals area: every session on the tmux servers the node follows
//! (the service user's own and other Unix users'), on the socket, as
//! commands and through [`parvi_term::Sessions`] for other areas.
//!
//! Its config section, `[terminals]`:
//!
//! ```toml
//! [terminals]
//! socket = "/run/tmux/default"   # the tmux server's socket path, or
//! socket_name = "work"           # its socket name (tmux -L)
//! users = { alice = "/tmp/tmux-1001/default" }  # other users' servers
//! launch = "launch.conf"         # the launcher's file
//! holder = "scratch"             # Windows: a scratch holder by this name
//! ```

#[cfg(all(test, unix))]
mod against_tmux;
mod agent_events;
mod channel;
mod commands;
mod flags;
mod frames;
mod hub;
mod launch;
mod launcher;
mod sessions;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use parvi_core::Error;
use parvi_node_kit::{Area, BoxFuture, Built, Channel, Client, Command, Failure, Listing, Setup};
use parvi_term::backend::tmux::Socket;
use parvi_term::{Engine, Sessions};
use serde::Deserialize;

use agent_events::hooks::{self, Hooks};
use commands::Ctx;
use hub::{Hub, Server};
use launcher::Launcher;

/// The area's name and config section.
pub const NAME: &str = "terminals";

/// How `bin/parvi` lists the area.
pub const LISTING: Listing = Listing {
    name: NAME,
    kinds: parvi_terminals_contract::KINDS,
    commands: &[
        "session_list",
        "session_read",
        "session_send",
        "session_launch",
        "session_copy",
        "session_history",
        "session_search",
        "session_keys",
        "session_rename",
        "session_kill",
        "session_mouse",
        "session_seen",
    ],
    build,
};

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    /// The tmux server's socket path (`-S`).
    socket: Option<PathBuf>,
    /// Or its socket name (`-L`).
    socket_name: Option<String>,
    /// Other Unix users' tmux servers: `name = socket path`.
    users: BTreeMap<String, PathBuf>,
    /// The launcher's file; `launch.conf` in the config folder if absent.
    launch: Option<PathBuf>,
    /// On Windows, a scratch session holder by this name instead of the
    /// user's own.
    holder: Option<String>,
}

struct Terminals {
    ctx: Arc<Ctx>,
}

/// Builds the area from `[terminals]`, and provides the node's
/// [`parvi_term::Sessions`].
pub fn build(setup: &Setup) -> Result<Built> {
    let section: Section = setup.config.section(NAME)?;
    if section.socket.is_some() && section.socket_name.is_some() {
        bail!("{NAME}: give socket or socket_name, not both");
    }
    for name in section.users.keys() {
        launch::check_user(name).with_context(|| format!("{NAME}.users"))?;
    }
    let launch_path = match &section.launch {
        Some(path) => setup.config.path(path),
        None => setup.dir.join("launch.conf"),
    };
    let launcher = Arc::new(Launcher::load(launch_path)?);
    let hub = Hub::new(servers(&section, setup)?)?;
    let hooks = std::env::current_exe()
        .context("finding this binary")
        .and_then(|exe| {
            Hooks::new(
                &exe,
                setup.address,
                setup.dir.join("agent-hooks"),
                hooks::codex_config().context("no home folder for Codex's config")?,
            )
        })
        .map_err(|error| format!("{error:#}"));
    if let Err(error) = &hooks {
        eprintln!("parvi: launches run without completion hooks: {error}");
    }
    let hub = Arc::new(hub.with_hooks(hooks));
    setup
        .provided
        .provide::<dyn Sessions>(Arc::new(sessions::Provided {
            hub: hub.clone(),
            launcher: launcher.clone(),
        }))?;
    Built::now(Terminals {
        ctx: Arc::new(Ctx {
            node: setup.node.id.clone(),
            hub,
            launcher,
        }),
    })
}

/// The servers `[terminals]` names, the service user's own first.
fn servers(section: &Section, setup: &Setup) -> Result<Vec<Server>> {
    let socket = match (&section.socket, &section.socket_name) {
        (Some(path), _) => Socket::Path(setup.config.path(path)),
        (None, Some(name)) => Socket::Name(name.clone()),
        (None, None) => Socket::Default,
    };
    if cfg!(windows) {
        if !section.users.is_empty() || !matches!(socket, Socket::Default) {
            bail!("{NAME}: tmux sockets and users are unavailable on Windows");
        }
        return Ok(vec![Server {
            user: None,
            engine: Arc::new(own_engine(section.holder.as_deref())?),
        }]);
    }
    if section.holder.is_some() {
        bail!("{NAME}: a holder is only for Windows");
    }
    let users = section
        .users
        .iter()
        .map(|(user, path)| (Some(user.clone()), Socket::Path(setup.config.path(path))));
    Ok(std::iter::once((None, socket))
        .chain(users)
        .map(|(user, socket)| Server {
            user,
            engine: Arc::new(Engine::start(socket)),
        })
        .collect())
}

#[cfg(windows)]
fn own_engine(holder: Option<&str>) -> Result<Engine> {
    use parvi_term::backend::holder::Holder;
    let holder = match holder {
        Some(name) => Holder::scratch(name)?,
        None => Holder::for_user()?,
    };
    Ok(Engine::start(holder))
}

#[cfg(not(windows))]
fn own_engine(_holder: Option<&str>) -> Result<Engine> {
    Ok(Engine::start(Socket::Default))
}

impl Area for Terminals {
    fn routes(&self) -> Router {
        Router::new()
            .route("/api/terminal/windows", get(windows))
            .with_state(self.ctx.hub.clone())
            .merge(agent_events::routes(self.ctx.hub.clone()))
            .merge(flags::routes(self.ctx.launcher.clone()))
    }

    fn commands(&self) -> Vec<Command> {
        commands::commands(&self.ctx)
    }

    fn channel(&self, client: Client) -> Option<Box<dyn Channel>> {
        let (hub, launcher) = (self.ctx.hub.clone(), self.ctx.launcher.clone());
        Some(Box::new(channel::Terminal::new(client, hub, launcher)))
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(self.ctx.hub.stop())
    }
}

#[derive(Deserialize)]
struct WindowQuery {
    session: String,
}

/// `GET /api/terminal/windows?session=<id>`: a session's windows and the
/// pane each shows, read when its picker opens.
async fn windows(State(hub): State<Arc<Hub>>, Query(query): Query<WindowQuery>) -> Response {
    match hub.windows(&query.session).await {
        Ok(windows) => axum::Json(windows).into_response(),
        Err(error) => Failure(
            StatusCode::BAD_REQUEST,
            Error::new(
                parvi_terminals_contract::COMMAND_FAILED,
                format!("{error:#}"),
            ),
        )
        .into_response(),
    }
}
