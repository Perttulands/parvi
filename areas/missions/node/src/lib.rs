//! The node-side mission service.
mod channel;
mod routes;
mod run_routes;

use anyhow::{Context, Result};
use axum::Router;
use parvi_missions::{Coordinator, LocalSeats, RunSettings, SeatExecutor, Store, TranscriptRoots};
use parvi_node_kit::{Area, BoxFuture, Built, Channel, Client, Listing, Setup};
use parvi_term::Sessions;
use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::broadcast;

pub const LISTING: Listing = Listing {
    name: "missions",
    kinds: parvi_missions_contract::KINDS,
    commands: &[],
    build,
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    dir: Option<PathBuf>,
}

#[derive(Clone)]
struct Missions {
    store: Store,
    coordinator: Coordinator,
    changes: broadcast::Sender<()>,
}
impl Missions {
    fn changed(&self) {
        let _ = self.changes.send(());
    }
}

pub fn build(setup: &Setup) -> Result<Built> {
    let section: Section = setup.config.section("missions")?;
    let root = section
        .dir
        .map(|dir| setup.config.path(&dir))
        .or_else(Store::default_root)
        .context("missions needs a state directory")?;
    let node = setup.node.id.to_string();
    let events = setup.events.clone();
    Built::then(move |provided| {
        let sessions = provided
            .get::<dyn Sessions>()
            .context("missions needs sessions")?;
        let seats = LocalSeats::new(sessions, &root, node, TranscriptRoots::from_host());
        let executor = SeatExecutor::new(Arc::new(seats), tokio::runtime::Handle::current());
        let settings = RunSettings {
            on_block: Some(parvi_missions::OnBlock(Arc::new(move |run, reason| {
                events.record(
                    "mission",
                    true,
                    format!(
                        "Run {} of {} blocked: {reason}",
                        run.run_id, run.mission_slug
                    ),
                )
            }))),
            ..Default::default()
        };
        let coordinator = Coordinator::open(&root, settings, Box::new(executor))?;
        Ok(Arc::new(Missions {
            store: Store::new(root),
            coordinator,
            changes: broadcast::channel(32).0,
        }))
    })
}
impl Area for Missions {
    fn routes(&self) -> Router {
        routes::routes(self.clone()).merge(run_routes::routes(self.coordinator.clone()))
    }
    fn channel(&self, _: Client) -> Option<Box<dyn Channel>> {
        Some(Box::new(channel::Connection::new(self.clone())))
    }
    fn stop(&self) -> BoxFuture<'_, ()> {
        let coordinator = self.coordinator.clone();
        Box::pin(async move {
            let _ = tokio::task::spawn_blocking(move || {
                coordinator.shutdown(Instant::now() + Duration::from_secs(10))
            })
            .await;
        })
    }
}
