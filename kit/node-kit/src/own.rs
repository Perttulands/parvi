//! The node's own commands and routes, which every node has whatever areas
//! it serves: no area may list one ([`crate::listing::check`]). They show its
//! config, and what the apps connected to it show (view control).

use std::sync::Arc;

use axum::Router;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use serde_json::{Value, json};

use crate::Command;
use parvi_core::NodeId;

use crate::config::Config;
use crate::serve::Failure;
use crate::view::{self, Views};

/// The names of the node's own commands.
pub const COMMANDS: &[&str] = &["config_show", "view_show", "view_open"];

/// What the node's own commands and routes read.
pub(crate) struct Own {
    pub config: Config,
    /// Every listed area's name: the sections the config may have.
    pub areas: Vec<&'static str>,
    pub node: NodeId,
    /// The apps connected to this node, for view control.
    pub views: Arc<Views>,
}

impl Own {
    fn config(&self, section: Option<&str>) -> Result<Value, parvi_core::Error> {
        self.config.show(section, &self.areas)
    }
}

pub(crate) fn commands(own: &Arc<Own>) -> Vec<Command> {
    let shown = own.clone();
    let mut commands = vec![Command::new(
        "config_show",
        "The node's config file and each section's keys and values as the node loaded them. \
         An agent edits the file; restarting the node applies it, and sessions survive.",
        || {
            json!({
                "type": "object",
                "properties": {
                    "section": { "type": "string", "description": "Only this section." },
                },
            })
        },
        move |args: Value| {
            let answer = shown.config(args["section"].as_str());
            async move { answer }
        },
    )];
    commands.extend(view::commands(&own.views, own.node.clone()));
    commands
}

#[derive(serde::Deserialize)]
struct Section {
    section: Option<String>,
}

/// `GET /api/config[?section=]`: what `config_show` answers.
pub(crate) fn routes(own: Arc<Own>) -> Router {
    Router::new().route(
        "/api/config",
        get(move |Query(q): Query<Section>| async move {
            match own.config(q.section.as_deref()) {
                Ok(answer) => axum::Json(answer).into_response(),
                Err(e) => Failure(StatusCode::NOT_FOUND, e).into_response(),
            }
        }),
    )
}
