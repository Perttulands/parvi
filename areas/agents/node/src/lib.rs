//! Reports the node's agent tools and skills, and starts checkout syncs.
mod checkout;
mod home;
mod report;
mod run;
mod skills;
mod sync;
mod tools;

use anyhow::{Context, Result};
use parvi_agents_contract::{FAILED, REPORT, SYNC, SYNCED, WATCH};
use parvi_core::Error;
use parvi_node_kit::args::{object, text};
use parvi_node_kit::{Area, BoxFuture, Built, Channel, Client, Command, Listing, Setup};
use parvi_term::Sessions;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub use checkout::Fetch;
pub use home::Home;
pub use report::{Report, gather};
pub use sync::sync;

pub const NAME: &str = "agents";
pub const LISTING: Listing = Listing {
    name: NAME,
    kinds: &[0x80..=0x8f],
    commands: &["agents_show", "agents_sync"],
    build,
};

struct AgentsArea {
    home: Option<Home>,
    sessions: Arc<dyn Sessions>,
    node: String,
}

pub fn build(setup: &Setup) -> Result<Built> {
    let home = Home::from_env();
    let node = setup.node.id.to_string();
    Built::then(move |provided| {
        Ok(Arc::new(AgentsArea {
            home,
            node,
            sessions: provided
                .get::<dyn Sessions>()
                .context("agents sync needs sessions")?,
        }))
    })
}

impl Area for AgentsArea {
    fn channel(&self, _: Client) -> Option<Box<dyn Channel>> {
        let (out, incoming) = mpsc::channel(32);
        Some(Box::new(Connection {
            home: self.home.clone(),
            sessions: self.sessions.clone(),
            out,
            incoming,
            watch: None,
        }))
    }
    fn commands(&self) -> Vec<Command> {
        let (home, node) = (self.home.clone(), self.node.clone());
        let show = Command::new(
            "agents_show",
            "Report this node's agent tools, skills and checkouts.",
            || object(json!({}), &[]),
            move |_| {
                let (home, node) = (home.clone(), node.clone());
                async move {
                    let home =
                        home.ok_or_else(|| Error::new(FAILED, "this node has no home folder"))?;
                    let mut report = gather(&home, Fetch::Fetching).await;
                    report.fetch(&home).await;
                    let mut value: Value = serde_json::from_str(&report.json(&home))
                        .map_err(|e| Error::new(FAILED, e.to_string()))?;
                    value["node"] = json!(node);
                    Ok(value)
                }
            },
        );
        let (home, sessions, node) = (self.home.clone(), self.sessions.clone(), self.node.clone());
        let sync = Command::new(
            "agents_sync",
            "Sync a skills checkout in a terminal session.",
            || object(json!({"checkout":{"type":"string"}}), &["checkout"]),
            move |args| {
                let (home, sessions, node) = (home.clone(), sessions.clone(), node.clone());
                async move {
                    let home =
                        home.ok_or_else(|| Error::new(FAILED, "this node has no home folder"))?;
                    let checkout =
                        text(&args, "checkout").map_err(|e| Error::new(FAILED, e.to_string()))?;
                    let session = sync::sync(&home, sessions.as_ref(), checkout)
                        .await
                        .map_err(|e| Error::new(FAILED, format!("{e:#}")))?;
                    Ok(json!({"node":node,"session":session}))
                }
            },
        );
        vec![show, sync]
    }
}
struct Connection {
    home: Option<Home>,
    sessions: Arc<dyn Sessions>,
    out: mpsc::Sender<Vec<u8>>,
    incoming: mpsc::Receiver<Vec<u8>>,
    watch: Option<JoinHandle<()>>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(task) = self.watch.take() {
            task.abort();
        }
    }
}
impl Channel for Connection {
    fn receive(&mut self, frame: Vec<u8>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Some(home) = self.home.clone() else {
                return;
            };
            match frame.as_slice() {
                [WATCH, on] => {
                    if let Some(task) = self.watch.take() {
                        task.abort();
                    }
                    if *on != 0 {
                        let out = self.out.clone();
                        self.watch = Some(tokio::spawn(async move {
                            let mut report = gather(&home, Fetch::Fetching).await;
                            let mut frame = vec![REPORT];
                            frame.extend(report.json(&home).as_bytes());
                            if out.send(frame).await.is_err() || report.checkouts.is_empty() {
                                return;
                            }
                            report.fetch(&home).await;
                            let mut frame = vec![REPORT];
                            frame.extend(report.json(&home).as_bytes());
                            let _ = out.send(frame).await;
                        }));
                    }
                }
                [SYNC, id @ ..] if id.len() >= 4 => {
                    let number = &id[..4];
                    let Ok(checkout) = std::str::from_utf8(&id[4..]) else {
                        return;
                    };
                    let (number, checkout, out, sessions) = (
                        number.to_vec(),
                        checkout.to_owned(),
                        self.out.clone(),
                        self.sessions.clone(),
                    );
                    tokio::spawn(async move {
                        let answer = sync::sync(&home, sessions.as_ref(), &checkout).await;
                        let mut frame = vec![SYNCED];
                        frame.extend(number);
                        frame.push(u8::from(answer.is_ok()));
                        frame.extend(
                            match answer {
                                Ok(session) => session,
                                Err(error) => {
                                    serde_json::to_string(&Error::new(FAILED, format!("{error:#}")))
                                        .expect("error serializes")
                                }
                            }
                            .as_bytes(),
                        );
                        let _ = out.send(frame).await;
                    });
                }
                _ => {}
            }
        })
    }
    fn next(&mut self) -> BoxFuture<'_, Option<Vec<u8>>> {
        Box::pin(self.incoming.recv())
    }
}
