//! The same definitions provide the CLI verbs and MCP tools.
use crate::{Files, FilesError};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use parvi_core::Error;
use parvi_node_kit::{
    Command,
    args::{flag, maybe_text, object, text},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

fn bad(e: impl std::fmt::Display) -> Error {
    Error::new("bad_request", e.to_string())
}
fn path(args: &Value, key: &str, home: Option<&Path>) -> Result<String, Error> {
    let raw = text(args, key).map_err(bad)?;
    let p = Path::new(raw);
    let expanded = if let Ok(rest) = p.strip_prefix("~") {
        home.ok_or_else(|| bad("home is unavailable"))?.join(rest)
    } else {
        p.to_path_buf()
    };
    expanded
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| bad("path is not UTF-8"))
}

pub fn commands(files: Arc<Files>, home: Option<PathBuf>) -> Vec<Command> {
    let defs: &[(&str, &str, &[&str], &[&str])] = &[
        ("file_list", "List roots or a folder.", &["path"], &[]),
        (
            "file_read",
            "Read a text file and its version.",
            &["path"],
            &["path"],
        ),
        (
            "file_diff",
            "Show a file's diff against HEAD.",
            &["path"],
            &["path"],
        ),
        (
            "file_projects",
            "List repositories beneath configured roots.",
            &[],
            &[],
        ),
        (
            "file_download",
            "Read a file as base64 bytes.",
            &["path"],
            &["path"],
        ),
        (
            "file_upload",
            "Create a file from base64 bytes; --replace overwrites it.",
            &["path", "data", "replace"],
            &["path", "data"],
        ),
        (
            "file_write",
            "Create or save a text file.",
            &["path", "text", "base", "new"],
            &["path", "text"],
        ),
        ("file_mkdir", "Create a folder.", &["path"], &["path"]),
        (
            "file_move",
            "Move or rename a file or folder.",
            &["path", "to"],
            &["path", "to"],
        ),
        (
            "file_delete",
            "Delete a file or folder.",
            &["path"],
            &["path"],
        ),
        (
            "file_find",
            "Find names under a folder.",
            &["path", "query"],
            &["path", "query"],
        ),
        (
            "file_search",
            "Find text under a folder.",
            &["path", "query"],
            &["path", "query"],
        ),
        (
            "file_copy",
            "Copy to another folder on this node.",
            &["path", "to", "to_node"],
            &["path", "to"],
        ),
        (
            "file_resume",
            "Resume a failed local copy.",
            &["id"],
            &["id"],
        ),
        (
            "file_transfer",
            "Read local copy progress.",
            &["id"],
            &["id"],
        ),
    ];
    defs.iter()
        .map(|&(name, description, props, required)| {
            let props = props.to_vec();
            let required = required.to_vec();
            let files = files.clone();
            let home = home.clone();
            Command::new(
                name,
                description,
                move || {
                    let mut fields = serde_json::Map::new();
                    for key in &props {
                        fields.insert(
                            (*key).into(),
                            if *key == "new" || *key == "replace" {
                                json!({"type":"boolean", "default":false})
                            } else {
                                json!({"type":"string"})
                            },
                        );
                    }
                    object(Value::Object(fields), &required)
                },
                move |args| {
                    let files = files.clone();
                    let home = home.clone();
                    async move { run(name, files, home, args).await }
                },
            )
        })
        .collect()
}

async fn run(
    name: &str,
    files: Arc<Files>,
    home: Option<PathBuf>,
    args: Value,
) -> Result<Value, Error> {
    if name == "file_copy" || name == "file_resume" || name == "file_transfer" {
        let id = if name == "file_copy" {
            let from = path(&args, "path", home.as_deref())?;
            let to = path(&args, "to", home.as_deref())?;
            let target = maybe_text(&args, "to_node")
                .map_err(bad)?
                .unwrap_or(&files.node);
            files.copy(&from, target, &to).map_err(Error::from)?
        } else {
            text(&args, "id").map_err(bad)?.to_owned()
        };
        if name == "file_resume" {
            files.resume(&id).map_err(|e| match e {
                FilesError::NotFound => {
                    Error::new("not_found", format!("no transfer {id} on this node"))
                }
                other => Error::from(other),
            })?;
        }
        if name != "file_transfer" {
            let mut watch = crate::bridge::watch(&files, &id)
                .ok_or_else(|| Error::from(FilesError::NotFound))?;
            let _ = tokio::time::timeout(std::time::Duration::from_secs(60), async {
                while watch.borrow_and_update().state == "running" {
                    if watch.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
        }
        let progress = files
            .transfer(&id)
            .ok_or_else(|| Error::from(FilesError::NotFound))?;
        if name != "file_transfer"
            && let Some(error) = &progress.error
        {
            return Err(Error::new(
                "copy_failed",
                format!("{error} (transfer {id}: file resume {id} goes on from here)"),
            ));
        }
        return Ok(json!({"node":files.node,"id":id,"to_node":files.node,"transfer":progress}));
    }
    let name = name.to_owned();
    tokio::task::spawn_blocking(move || work(&name, &files, home.as_deref(), &args))
        .await
        .map_err(|e| Error::new("io_error", e.to_string()))?
}

fn work(name: &str, files: &Files, home: Option<&Path>, args: &Value) -> Result<Value, Error> {
    let node = &files.node;
    if name == "file_list" && maybe_text(args, "path").map_err(bad)?.is_none() {
        return Ok(json!({"node":node,"roots":files.roots()}));
    }
    if name == "file_projects" {
        return serde_json::to_value(crate::disk::projects(&files.roots)?).map_err(bad);
    }
    let at = path(args, "path", home)?;
    match name {
        "file_list" => {
            let l = files.list(&at)?;
            Ok(json!({"node":node,"path":l.path,"entries":l.entries}))
        }
        "file_read" => {
            let (bytes, version) = files.read(&at, crate::routes::MAX_BYTES)?;
            let shown = &bytes[..bytes.len().min(256 * 1024)];
            let content = match std::str::from_utf8(shown) {
                Ok(s) => s,
                Err(e) if e.error_len().is_none() => {
                    std::str::from_utf8(&shown[..e.valid_up_to()]).unwrap()
                }
                Err(_) => return Err(bad("file is not text")),
            };
            Ok(
                json!({"node":node,"path":at,"size":bytes.len(),"version":version,"truncated":shown.len()<bytes.len(),"text":content}),
            )
        }
        "file_diff" => serde_json::to_value(files.diff(&at)?).map_err(bad),
        "file_download" => {
            let (bytes, version) = files.read(&at, crate::routes::MAX_BYTES)?;
            Ok(
                json!({"node":node,"path":at,"size":bytes.len(),"version":version,"data":STANDARD.encode(bytes)}),
            )
        }
        "file_upload" => {
            let bytes = STANDARD
                .decode(text(args, "data").map_err(bad)?)
                .map_err(bad)?;
            if bytes.len() > crate::routes::MAX_BYTES as usize {
                return Err(FilesError::TooLarge.into());
            }
            if flag(args, "replace", false).map_err(bad)? {
                files.save(&at, &bytes, None)?;
            } else {
                files.create(&at, &bytes)?;
            }
            Ok(json!({"node":node,"path":at,"size":bytes.len(),"version":crate::version(&bytes)}))
        }
        "file_write" => {
            let data = text(args, "text").map_err(bad)?.as_bytes();
            let base = maybe_text(args, "base").map_err(bad)?;
            if flag(args, "new", false).map_err(bad)? {
                files.create(&at, data)?;
            } else {
                files.save(&at, data, base)?;
            }
            Ok(json!({"node":node,"path":at,"version":crate::version(data)}))
        }
        "file_mkdir" => {
            files.create_folder(&at)?;
            Ok(json!({"node":node,"path":at}))
        }
        "file_move" => {
            let to = path(args, "to", home)?;
            files.rename(&at, &to)?;
            Ok(json!({"node":node,"path":to}))
        }
        "file_delete" => {
            files.delete(&at)?;
            Ok(json!({"node":node,"path":at,"deleted":true}))
        }
        "file_find" | "file_search" => {
            let found = files.find(
                &at,
                text(args, "query").map_err(bad)?,
                name == "file_search",
            )?;
            let mut v = serde_json::to_value(found).map_err(bad)?;
            v["node"] = json!(node);
            Ok(v)
        }
        _ => Err(bad("unknown command")),
    }
}
