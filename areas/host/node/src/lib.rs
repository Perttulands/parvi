//! Host readings and service management.
pub mod host;
use parvi_core::Error;
use parvi_node_kit::{Area, BoxFuture, Built, Channel, Client, Command, Listing, Setup};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle};

pub const LISTING: Listing = Listing {
    name: "host",
    kinds: &[parvi_host_contract::HOST..=parvi_host_contract::HOST],
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
        ["host_show", "service_list", "service_show", "service_start", "service_stop", "service_restart"].into_iter().map(|name| {
   let (services, events, node) = (self.services.clone(), self.events.clone(), self.node.clone());
   Command::new(name, name, move || if name == "host_show" || name == "service_list" { json!({"type":"object","properties":{}}) } else { json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}) }, move |args: Value| { let (services, events, node) = (services.clone(), events.clone(), node.clone()); async move {
    let bad = |e: String| Error::new(parvi_host_contract::BAD_REQUEST, e);
    if name == "host_show" { let mut v = host::now(Duration::from_millis(500), &events).await; v["node"] = json!(node); return Ok(v); }
    if name == "service_list" { return Ok(json!({"node":node,"services":services.list().await.map_err(|e|e.error())?})); }
    let id = args["id"].as_str().ok_or_else(|| bad("id is required".into()))?;
    if name == "service_show" { return Ok(json!({"node":node,"service":services.one(id).await.map_err(|e|e.error())?})); }
    let action = name.strip_prefix("service_").unwrap();
    let (service, errors, outcome) = services.act(id, action).await.map_err(|e|e.error())?;
    if !errors.is_empty() { return Err(Error::new(parvi_host_contract::SERVICE_FAILED, errors.join("; "))); }
    Ok(json!({"node":node,"service":service,"errors":errors,"outcome":outcome}))
   }})
  }).collect()
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
