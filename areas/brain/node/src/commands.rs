use crate::brain::Brain;
use parvi_brain_contract::{BAD_REQUEST, BRAIN_ERROR, NOT_FOUND, UNAVAILABLE};
use parvi_core::Error;
use parvi_node_kit::{
    Command,
    args::{maybe_text, object, text},
};
use serde_json::json;
use std::sync::Arc;

pub fn commands(
    brain: Arc<Brain>,
    node: String,
    sessions: Option<Arc<dyn parvi_term::Sessions>>,
) -> Vec<Command> {
    let search = brain.clone();
    let search_node = node.clone();
    let read = brain.clone();
    let read_node = node.clone();
    let save = brain.clone();
    let save_node = node.clone();
    let collections = brain.clone();
    let collections_node = node.clone();
    let sending = brain.clone();
    let sending_node = node.clone();
    vec![
        Command::new(
            "brain_search",
            "Search the brain for pages.",
            || object(json!({"query": {"type":"string"}}), &["query"]),
            move |args| {
                let brain = search.clone();
                let node = search_node.clone();
                async move {
                    let query =
                        text(&args, "query").map_err(|e| Error::new(BAD_REQUEST, e.to_string()))?;
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
                let brain = read.clone();
                let node = read_node.clone();
                async move {
                    let path =
                        text(&args, "path").map_err(|e| Error::new(BAD_REQUEST, e.to_string()))?;
                    let page = brain
                        .page(path)
                        .await
                        .map_err(|e| Error::new(crate::brain::refusal_code(e.status), e.message))?;
                    let mut value = serde_json::to_value(page)
                        .map_err(|e| Error::new(BRAIN_ERROR, e.to_string()))?;
                    value["node"] = json!(node);
                    Ok(value)
                }
            },
        ),
        Command::new(
            "brain_save",
            "Save a page as a commit; base is the page's last commit hash from brain read. Omit base to save the current clean page.",
            || {
                object(
                    json!({
                        "path": {"type":"string"},
                        "text": {"type":"string"},
                        "summary": {"type":"string"},
                        "base": {"type":"string", "description":"Last commit hash from brain read; empty for an uncommitted page."}
                    }),
                    &["path", "text"],
                )
            },
            move |args| {
                let brain = save.clone();
                let node = save_node.clone();
                async move {
                    let bad = |e: anyhow::Error| Error::new(BAD_REQUEST, e.to_string());
                    let path = text(&args, "path").map_err(bad)?.to_string();
                    let content = text(&args, "text").map_err(bad)?.to_string();
                    let summary = maybe_text(&args, "summary")
                        .map_err(bad)?
                        .unwrap_or("")
                        .to_string();
                    let base = maybe_text(&args, "base").map_err(bad)?.map(str::to_owned);
                    let task = tokio::spawn(async move {
                        brain.save(&path, &content, &summary, base.as_deref()).await
                    });
                    let commit = task
                        .await
                        .map_err(|e| Error::new(BRAIN_ERROR, format!("Save stopped: {e}")))?
                        .map_err(|e| Error::new(crate::brain::refusal_code(e.status), e.message))?;
                    Ok(json!({"node": node, "commit": commit}))
                }
            },
        ),
        Command::new(
            "brain_collections",
            "List the brain's collections and page counts.",
            || object(json!({}), &[]),
            move |_| {
                let brain = collections.clone();
                let node = collections_node.clone();
                async move {
                    let collections = brain
                        .collections()
                        .await
                        .map_err(|e| Error::new(crate::brain::refusal_code(e.status), e.message))?;
                    Ok(json!({"node": node, "collections": collections}))
                }
            },
        ),
        Command::new(
            "brain_changes",
            "List recent commits, newest first, with their pages.",
            || {
                object(
                    json!({"limit": {"type":"integer", "minimum":1, "description":"Maximum commits (default 30, maximum 200)."}}),
                    &[],
                )
            },
            move |args| {
                let brain = brain.clone();
                let node = node.clone();
                async move {
                    let limit = match args.get("limit") {
                        None | Some(serde_json::Value::Null) => None,
                        Some(value) => Some(
                            value
                                .as_u64()
                                .filter(|n| *n > 0)
                                .and_then(|n| usize::try_from(n).ok())
                                .ok_or_else(|| {
                                    Error::new(BAD_REQUEST, "limit must be a positive whole number")
                                })?,
                        ),
                    };
                    let read = brain
                        .changes(limit)
                        .await
                        .map_err(|e| Error::new(crate::brain::refusal_code(e.status), e.message))?;
                    let changes: Vec<_> = read.found.iter().map(|change| json!({
                        "hash": change.commit.hash, "time": change.commit.time, "author": change.commit.author,
                        "message": change.commit.message,
                        "paths": change.pages.iter().map(|page| page.path.as_str()).collect::<Vec<_>>().join(", "),
                        "pages": change.pages,
                    })).collect();
                    Ok(json!({"node": node, "changes": changes, "error": read.error}))
                }
            },
        ),
        Command::new(
            "brain_send",
            "Send a page reference to a session; use user/name if the name is ambiguous.",
            || {
                object(
                    json!({"path": {"type":"string"}, "session": {"type":"string"}}),
                    &["path", "session"],
                )
            },
            move |args| {
                let brain = sending.clone();
                let node = sending_node.clone();
                let sessions = sessions.clone();
                async move {
                    let path =
                        text(&args, "path").map_err(|e| Error::new(BAD_REQUEST, e.to_string()))?;
                    let target = text(&args, "session")
                        .map_err(|e| Error::new(BAD_REQUEST, e.to_string()))?;
                    let page = brain
                        .page(path)
                        .await
                        .map_err(|e| Error::new(crate::brain::refusal_code(e.status), e.message))?;
                    let sessions = sessions.ok_or_else(|| {
                        Error::new(
                            UNAVAILABLE,
                            "No terminal sessions are available on this node",
                        )
                    })?;
                    let found: Vec<_> = sessions
                        .list()
                        .into_iter()
                        .filter(|s| {
                            s.id == target
                                || (s.user.is_none() && s.name == target)
                                || s.user
                                    .as_ref()
                                    .is_some_and(|user| format!("{user}/{}", s.name) == target)
                        })
                        .collect();
                    let session = match found.as_slice() {
                        [session] => session,
                        [] => return Err(Error::new(NOT_FOUND, format!("No session {target}"))),
                        _ => {
                            return Err(Error::new(
                                BAD_REQUEST,
                                format!("Session {target} is ambiguous"),
                            ));
                        }
                    };
                    let pane = sessions
                        .active_pane(&session.id)
                        .await
                        .map_err(|e| Error::new(BRAIN_ERROR, e.to_string()))?;
                    let reference = format!("brain page {}", page.file);
                    sessions
                        .deliver(pane, reference.clone(), true)
                        .await
                        .map_err(|e| Error::new(BRAIN_ERROR, e.to_string()))?
                        .await
                        .map_err(|e| Error::new(BRAIN_ERROR, e.to_string()))?;
                    Ok(
                        json!({"node": node, "session": session.id, "reference": reference, "sent": true}),
                    )
                }
            },
        ),
    ]
}
