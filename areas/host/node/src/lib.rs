//! Host readings and service management.
mod commands;
pub mod host;
use parvi_node_kit::{Area, BoxFuture, Built, Channel, Client, Command, Listing, Setup};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
use tokio::{sync::mpsc, task::JoinHandle};

pub const LISTING: Listing = Listing {
    name: "host",
    kinds: &[parvi_host_contract::HOST..=parvi_host_contract::HOST_EVENT],
    commands: &[
        "host_show",
        "service_list",
        "service_show",
        "service_start",
        "service_stop",
        "service_restart",
    ],
    build,
};
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    catalog: Option<PathBuf>,
}
struct HostArea {
    host: Arc<host::Host>,
    services: Arc<host::services::Services>,
    node: String,
    events: Arc<parvi_node_kit::events::Log>,
}
pub fn build(setup: &Setup) -> anyhow::Result<Built> {
    let section: Section = setup.config.section("host")?;
    let catalog = section
        .catalog
        .as_ref()
        .map(|p| setup.config.path(p))
        .unwrap_or_else(|| setup.dir.join("services.json"));
    Built::now(HostArea {
        host: Arc::new(host::Host::start(
            host::EVERY,
            host::HISTORY,
            setup.events.clone(),
        )),
        services: Arc::new(
            host::services::Services::new(catalog).with_events(setup.events.clone()),
        ),
        node: setup.node.id.to_string(),
        events: setup.events.clone(),
    })
}
impl Area for HostArea {
    fn routes(&self) -> axum::Router {
        host::services::routes(self.services.clone())
    }
    fn channel(&self, _: Client) -> Option<Box<dyn Channel>> {
        let (tx, rx) = mpsc::channel(32);
        Some(Box::new(Stream {
            host: self.host.clone(),
            tx,
            rx,
            task: None,
        }))
    }
    fn commands(&self) -> Vec<Command> {
        commands::commands(
            self.services.clone(),
            self.events.clone(),
            self.node.clone(),
        )
    }
}
struct Stream {
    host: Arc<host::Host>,
    tx: mpsc::Sender<Vec<u8>>,
    rx: mpsc::Receiver<Vec<u8>>,
    task: Option<JoinHandle<()>>,
}
impl Channel for Stream {
    fn receive(&mut self, frame: Vec<u8>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Ok(on) = host::decode_on(&frame) {
                if let Some(task) = self.task.take() {
                    task.abort();
                    let _ = task.await;
                }
                while self.rx.try_recv().is_ok() {}
                if on {
                    self.task = Some(tokio::spawn(self.host.stream(self.tx.clone())));
                }
            }
        })
    }
    fn next(&mut self) -> BoxFuture<'_, Option<Vec<u8>>> {
        Box::pin(self.rx.recv())
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
