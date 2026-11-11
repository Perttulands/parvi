//! Work's tracker readers, commands and watched snapshot channel, and
//! reference resolution for other areas ([`parvi_work_contract::Resolver`]).
#[cfg(test)]
mod test_support;
mod work;

use anyhow::Result;
use axum::Router;
use parvi_core::{Error, NodeId};
use parvi_node_kit::args::{flag, object, text};
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
            "List non-closed work items, including blocked and deferred; use --all to include closed and merged items.",
            || object(json!({"all": {"type":"boolean", "default":false}}), &[]),
            move |args| {
                let (work, node) = (work.clone(), node.clone());
                async move {
                    let all = flag(&args, "all", false)
                        .map_err(|e| Error::new("bad_request", e.to_string()))?;
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
                                if !all {
                                    omit_closed(&mut snapshot);
                                }
                                snapshot["items"] = json!(listed_items(&snapshot));
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
                    let item = work.item(&reference).await.map_err(|error| {
                        if error.downcast_ref::<work::MissingItem>().is_some() {
                            Error::new("not_found", error.to_string())
                        } else {
                            failed(error)
                        }
                    })?;
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
/// Filter only the command's copy of the board; the page keeps its full history.
fn omit_closed(snapshot: &mut serde_json::Value) {
    for project in snapshot["projects"].as_array_mut().into_iter().flatten() {
        for (list, status) in [("items", "status"), ("issues", "state"), ("pulls", "state")] {
            if let Some(items) = project
                .get_mut(list)
                .and_then(serde_json::Value::as_array_mut)
            {
                items.retain(|item| !matches!(item[status].as_str(), Some("closed" | "merged")));
            }
        }
    }
}
/// A flat index makes the board's items visible in the generic CLI renderer,
/// which otherwise shows only each project's scalar fields.
fn listed_items(snapshot: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut items = Vec::new();
    for project in snapshot["projects"].as_array().into_iter().flatten() {
        match project["tracker"].as_str() {
            Some("beads") => {
                for item in project["items"].as_array().into_iter().flatten() {
                    items.push(json!({"project": project["folder"], "id": item["id"], "reference": item["reference"], "title": item["title"], "status": item["status"]}));
                }
            }
            Some("github") => {
                for (key, kind) in [("issues", "issue"), ("pulls", "pull")] {
                    for item in project[key].as_array().into_iter().flatten() {
                        let repo = project["name"].as_str().unwrap_or("");
                        let number = item["number"].as_u64().unwrap_or_default();
                        items.push(json!({"project": repo, "id": number, "reference": format!("gh:{repo}#{number}"), "title": item["title"], "status": item["state"], "kind": kind}));
                    }
                }
            }
            _ => {}
        }
    }
    items
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

#[cfg(test)]
mod command_tests {
    use super::*;

    #[test]
    fn command_filter_keeps_active_items_and_draft_pulls() {
        let mut board = json!({"projects": [
            {"tracker":"beads", "items":[{"status":"open"},{"status":"in_progress"},{"status":"blocked"},{"status":"closed"}]},
            {"tracker":"github", "issues":[{"state":"open"},{"state":"closed"}], "pulls":[{"state":"draft"},{"state":"open"},{"state":"closed"},{"state":"merged"}]}
        ]});
        omit_closed(&mut board);
        assert_eq!(board["projects"][0]["items"].as_array().unwrap().len(), 3);
        assert_eq!(board["projects"][1]["issues"].as_array().unwrap().len(), 1);
        assert_eq!(board["projects"][1]["pulls"].as_array().unwrap().len(), 2);
    }
}
