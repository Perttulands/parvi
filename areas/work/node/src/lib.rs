//! Work's tracker readers, commands and watched snapshot channel, and
//! reference resolution for other areas ([`parvi_work_contract::Resolver`]).
#[cfg(test)]
mod test_support;
mod work;

use anyhow::Result;
use axum::Router;
use parvi_core::{Error, NodeId};
use parvi_node_kit::args::{object, text};
use parvi_node_kit::{Area, BoxFuture, Built, Channel, Client, Command, Listing, Setup};
use parvi_work_contract::{Resolver, Resolving};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
pub use work::{Bd, Gh};
use work::{Config, Work};

pub const LISTING: Listing = Listing {
    name: "work",
    kinds: parvi_work_contract::KINDS,
    commands: &["work_list", "work_show", "work_create"],
    build,
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    /// Project lines in the same form as the former work.conf.
    projects: Vec<String>,
}

pub fn build(setup: &Setup) -> Result<Built> {
    build_with(setup, Bd::default(), Gh::default())
}

/// Build with tracker programs supplied by a test or embedding node.
pub fn build_with(setup: &Setup, bd: Bd, gh: Gh) -> Result<Built> {
    let section: Section = setup.config.section("work")?;
    let config = Config::parse(&section.projects.join("\n"))?;
    let work = Work::new(config, setup.node.id.as_str(), gh, bd);
    setup
        .provided
        .provide::<dyn Resolver>(Arc::new(Provided(work.clone())))?;
    Built::now(WorkArea {
        work,
        node: setup.node.id.clone(),
    })
}

/// Work as other areas reach it ([`parvi_work_contract::Resolver`]).
struct Provided(Work);

impl Resolver for Provided {
    fn resolve<'a>(&'a self, written: &'a str) -> Resolving<'a> {
        Box::pin(async move {
            self.0
                .resolve(written)
                .await
                .map_err(|error| Error::new(parvi_work_contract::BAD_REQUEST, format!("{error:#}")))
        })
    }
}

struct WorkArea {
    work: Work,
    node: NodeId,
}
impl Area for WorkArea {
    fn routes(&self) -> Router {
        work::routes(self.work.clone())
    }
    fn commands(&self) -> Vec<Command> {
        let (work, node) = (self.work.clone(), self.node.clone());
        let list = Command::new(
            "work_list",
            "The Work board, read now.",
            || object(json!({}), &[]),
            move |_| {
                let (work, node) = (work.clone(), node.clone());
                async move {
                    let mut watching = work.subscribe();
                    work.refresh();
                    let read = async {
                        loop {
                            let mut snapshot =
                                serde_json::to_value(&*watching.next().await).map_err(failed)?;
                            if snapshot["projects"].as_array().is_some_and(|p| {
                                p.iter()
                                    .all(|p| p.get("synced").is_some() || p.get("error").is_some())
                            }) {
                                snapshot["node"] = json!(node);
                                return Ok(snapshot);
                            }
                        }
                    };
                    tokio::time::timeout(Duration::from_secs(30), read)
                        .await
                        .map_err(|_| Error::new("tracker_error", "work read timed out"))?
                }
            },
        );
        let (work, node) = (self.work.clone(), self.node.clone());
        let show = Command::new(
            "work_show",
            "One work item and its family.",
            || object(json!({"ref": {"type":"string"}}), &["ref"]),
            move |args| {
                let (work, node) = (work.clone(), node.clone());
                async move {
                    let written =
                        text(&args, "ref").map_err(|e| Error::new("bad_request", e.to_string()))?;
                    let reference = work.resolve(written).await.map_err(failed)?;
                    let item = work.item(&reference).await.map_err(failed)?;
                    let line = work.reference_line(&item).await;
                    Ok(json!({"node":node,"item":item,"line":line}))
                }
            },
        );
        let work = self.work.clone();
        let create = Command::new(
            "work_create",
            "File an item in a configured project.",
            || {
                object(
                    json!({"project":{"type":"string"}, "title":{"type":"string"}, "description":{"type":"string"}, "type":{"type":"string", "enum":["bug", "task", "feature"]}}),
                    &["project", "title", "description"],
                )
            },
            move |args| {
                let work = work.clone();
                async move {
                    let request = serde_json::from_value(args)
                        .map_err(|e| Error::new("bad_request", e.to_string()))?;
                    let reference = work.create(&request).await?;
                    Ok(json!({"reference": reference}))
                }
            },
        );
        vec![list, show, create]
    }
    fn channel(&self, _: Client) -> Option<Box<dyn Channel>> {
        Some(Box::new(Socket(work::Channel::new(self.work.clone()))))
    }
}
fn failed(e: impl std::fmt::Display) -> Error {
    Error::new("tracker_error", e.to_string())
}
struct Socket(work::Channel);
impl Channel for Socket {
    fn receive(&mut self, frame: Vec<u8>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.0.receive(&frame);
        })
    }
    fn next(&mut self) -> BoxFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move { Some(self.0.next().await) })
    }
}
