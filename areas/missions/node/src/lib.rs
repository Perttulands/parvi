//! The node-side mission service.
mod authoring;
mod channel;
mod commands;
mod routes;
mod run_routes;

use anyhow::{Context, Result};
use axum::Router;
use parvi_mission_engine::{
    Coordinator, LocalSeats, RunSettings, SeatExecutor, Store, TranscriptRoots,
};
use parvi_node_kit::{Area, BoxFuture, Built, Channel, Client, Listing, Setup};
use parvi_term::Sessions;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::broadcast;

pub const LISTING: Listing = Listing {
    name: "missions",
    kinds: parvi_missions_contract::KINDS,
    commands: commands::NAMES,
    build,
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    repositories: BTreeMap<String, PathBuf>,
    run_workspace_root: Option<PathBuf>,
}

#[derive(Clone)]
struct Missions {
    store: Store,
    coordinator: Coordinator,
    changes: broadcast::Sender<()>,
    resolver: Arc<dyn parvi_work_contract::Resolver>,
    node: String,
}
impl Missions {
    fn changed(&self) {
        let _ = self.changes.send(());
    }
}

pub fn build(setup: &Setup) -> Result<Built> {
    build_with_executor(setup, None)
}

/// Inject an executor for isolated test nodes.
pub fn build_with_executor(
    setup: &Setup,
    override_executor: Option<Box<dyn parvi_mission_engine::Executor>>,
) -> Result<Built> {
    let section: Section = setup.config.section("missions")?;
    let repositories: BTreeMap<String, PathBuf> = section
        .repositories
        .into_iter()
        .map(|(name, path)| (name, setup.config.path(&path)))
        .collect();
    let run_workspace_root = section
        .run_workspace_root
        .map(|path| setup.config.path(&path));
    let root = Store::root_in(&setup.dir);
    let node = setup.node.id.to_string();
    let environment = format!("{}/{}", setup.node.device, setup.node.environment.name());
    let events = setup.events.clone();
    Built::then(move |provided| {
        let sessions = provided
            .get::<dyn Sessions>()
            .context("missions needs sessions")?;
        let resolver = provided
            .get::<dyn parvi_work_contract::Resolver>()
            .context("missions needs Work reference resolution")?;
        let seats = LocalSeats::new(sessions, &root, node.clone(), TranscriptRoots::from_host())
            .with_repositories(repositories.clone());
        let executor = SeatExecutor::new(Arc::new(seats), tokio::runtime::Handle::current());
        let settings = RunSettings {
            environment,
            repositories,
            run_workspace_root,
            on_block: Some(parvi_mission_engine::OnBlock(Arc::new(
                move |run, reason| {
                    events.record(
                        "mission",
                        true,
                        format!(
                            "Run {} of {} blocked: {reason}",
                            run.run_id, run.mission_slug
                        ),
                    )
                },
            ))),
            ..Default::default()
        };
        let executor = override_executor.unwrap_or_else(|| Box::new(executor));
        let coordinator = Coordinator::open(&root, settings, executor)?;
        Ok(Arc::new(Missions {
            store: Store::new(root),
            coordinator,
            changes: broadcast::channel(32).0,
            resolver,
            node,
        }))
    })
}
impl Area for Missions {
    fn routes(&self) -> Router {
        routes::routes(self.clone()).merge(run_routes::routes(self.clone()))
    }
    fn commands(&self) -> Vec<parvi_node_kit::Command> {
        commands::commands(self)
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
