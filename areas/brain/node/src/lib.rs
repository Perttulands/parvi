//! Brain corpus, search, and commands.
pub mod brain;
mod commands;

use anyhow::Result;
use axum::Router;
use parvi_node_kit::{Area, Built, Command, Listing, Setup};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};

pub const LISTING: Listing = Listing {
    name: "brain",
    kinds: &[],
    commands: &["brain_search", "brain_read"],
    build,
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    corpus: Option<PathBuf>,
    citadel: Option<String>,
    author: Option<String>,
}

pub fn build(setup: &Setup) -> Result<Built> {
    let section: Section = setup.config.section("brain")?;
    let corpus = section.corpus.as_ref().map(|p| setup.config.path(p));
    let brain = brain::Brain::new(corpus.as_deref(), section.citadel.as_deref())?
        .with_author(section.author.as_deref())?;
    Built::now(BrainArea {
        brain: Arc::new(brain),
        node: setup.node.id.to_string(),
    })
}
struct BrainArea {
    brain: Arc<brain::Brain>,
    node: String,
}
impl Area for BrainArea {
    fn routes(&self) -> Router {
        brain::routes(self.brain.clone())
    }
    fn commands(&self) -> Vec<Command> {
        commands::commands(self.brain.clone(), self.node.clone())
    }
}
