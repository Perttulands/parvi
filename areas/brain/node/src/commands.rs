use crate::brain::Brain;
use parvi_core::Error;
use parvi_node_kit::{
    Command,
    args::{object, text},
};
use serde_json::json;
use std::sync::Arc;

pub fn commands(brain: Arc<Brain>, node: String) -> Vec<Command> {
    let search = brain.clone();
    let search_node = node.clone();
    vec![
        Command::new(
            "brain_search",
            "Search the brain for pages.",
            || object(json!({"query": {"type":"string"}}), &["query"]),
            move |args| {
                let brain = search.clone();
                let node = search_node.clone();
                async move {
                    let query = text(&args, "query")
                        .map_err(|e| Error::new("bad_request", e.to_string()))?;
                    let results = brain
                        .search(query)
                        .await
                        .map_err(|e| Error::new(crate::brain::refusal_code(e.status), e.message))?;
                    Ok(json!({"node": node, "results": results}))
                }
            },
        ),
        Command::new(
            "brain_read",
            "Read a brain page.",
            || object(json!({"path": {"type":"string"}}), &["path"]),
            move |args| {
                let brain = brain.clone();
                let node = node.clone();
                async move {
                    let path = text(&args, "path")
                        .map_err(|e| Error::new("bad_request", e.to_string()))?;
                    let page = brain
                        .page(path)
                        .await
                        .map_err(|e| Error::new(crate::brain::refusal_code(e.status), e.message))?;
                    let mut value = serde_json::to_value(page)
                        .map_err(|e| Error::new("brain_error", e.to_string()))?;
                    value["node"] = json!(node);
                    Ok(value)
                }
            },
        ),
    ]
}
