//! The scheduled area: tasks that paste a prompt into named sessions on a
//! schedule (#189), CHROTE's scheduler ported (`docs/scheduled-tasks.md`).
//! It serves CHROTE's HTTP API ([`parvi_scheduled_contract`]) and the
//! `scheduled_*` commands, and delivers through the node's
//! [`parvi_term::Sessions`], so a node needs an area providing them.
//!
//! What differs from CHROTE: it re-reads every task every 30 seconds. Here
//! the node keeps when each task is next due, and its one timer sleeps until
//! the earliest of them; with nothing scheduled there is no timer at all.
//! Task files are read when the API is asked and when a task falls due. A
//! task's targets are this node's sessions.
//!
//! A run is never repeated: it is written ahead into the task's file before
//! anything is sent, and nothing is sent if that write fails. A task whose
//! file will not read, whose run could not be written ahead or recorded, or
//! which the node finds still mid-run as it starts, is held back (`held` in
//! the API, an event in the node's log) until it is edited or resumed. A due
//! task another call holds waits for that claim to be given back, which
//! wakes the timer.
//!
//! Its config section, `[scheduled]`:
//!
//! ```toml
//! [scheduled]
//! dir = "scheduled-tasks"   # the tasks' folder; this name in the config folder if absent
//! ```

mod commands;
mod routes;
mod runner;
mod schedule;
mod service;
mod store;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::Router;
use parvi_node_kit::{Area, BoxFuture, Built, Command, Setup};
use parvi_term::Sessions;
use serde::Deserialize;
use tokio::task::JoinHandle;

use commands::Ctx;
use runner::SessionsRunner;
pub use service::{Create, Error, Invalid, Patch, Runner, Scheduled};

/// The area's name and config section.
pub const NAME: &str = "scheduled";

/// How long the timer waits for the sessions to be listed before firing
/// what fell due while the node was down.
const SESSIONS_LISTED_WITHIN: Duration = Duration::from_secs(3);

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    /// The tasks' folder. Set it only where this node's scheduler should
    /// fire them.
    dir: Option<PathBuf>,
}

struct ScheduledArea {
    ctx: Arc<Ctx>,
    /// The timer and the rename followers, stopped with the node.
    running: Mutex<Vec<JoinHandle<()>>>,
}

/// Builds the area from `[scheduled]`; once every area is built, reads the
/// tasks, follows session renames and starts the timer.
pub fn build(setup: &Setup) -> Result<Built> {
    let section: Section = setup.config.section(NAME)?;
    let dir = match &section.dir {
        Some(dir) => setup.config.path(dir),
        None => setup.dir.join("scheduled-tasks"),
    };
    let (node, events) = (setup.node.id.clone(), setup.events.clone());
    Built::then(move |provided| {
        let sessions = provided
            .get::<dyn Sessions>()
            .context("scheduled delivers into sessions")?;
        let scheduled = Arc::new(
            Scheduled::new(dir, Arc::new(SessionsRunner::new(sessions.clone())))
                .with_events(events),
        );
        scheduled.prime()?;
        // A renamed session keeps its tasks.
        let mut running = scheduled.follow_renames(sessions.as_ref());
        // Tasks that fell due while the node was down fire once tmux's
        // sessions are listed, not into an empty list.
        running.push(tokio::spawn({
            let (scheduled, mut listed) = (scheduled.clone(), sessions.watch());
            async move {
                let first = listed.changed();
                let _ = tokio::time::timeout(SESSIONS_LISTED_WITHIN, first).await;
                scheduled.run().await
            }
        }));
        Ok(Arc::new(ScheduledArea {
            ctx: Arc::new(Ctx { node, scheduled }),
            running: Mutex::new(running),
        }) as Arc<dyn Area>)
    })
}

impl Area for ScheduledArea {
    fn name(&self) -> &'static str {
        NAME
    }

    fn routes(&self) -> Router {
        routes::routes(self.ctx.scheduled.clone())
    }

    fn commands(&self) -> Vec<Command> {
        commands::commands(&self.ctx)
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            for handle in self.running.lock().unwrap().drain(..) {
                handle.abort();
            }
        })
    }
}
