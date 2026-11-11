//! Help catalogue for configured harnesses.
use crate::launcher::Launcher;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use parvi_core::Error;
use parvi_node_kit::Failure;
use parvi_terminals_contract::Flag;
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{io::AsyncReadExt, sync::Mutex};

struct Catalogue {
    launcher: Arc<Launcher>,
    cache: Mutex<HashMap<PathBuf, (SystemTime, Vec<Flag>)>>,
}
pub fn routes(launcher: Arc<Launcher>) -> Router {
    Router::new()
        .route("/api/launch/flags", get(flags))
        .with_state(Arc::new(Catalogue {
            launcher,
            cache: Mutex::new(HashMap::new()),
        }))
}
#[derive(Deserialize)]
struct Request {
    harness: String,
}
fn failure(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    Failure(status, Error::new(code, message)).into_response()
}
async fn flags(
    State(catalogue): State<Arc<Catalogue>>,
    Query(request): Query<Request>,
) -> Response {
    match get_flags(&catalogue, &request.harness).await {
        Ok(flags) => Json(flags).into_response(),
        Err((status, code, message)) => failure(status, code, message),
    }
}
type FlagError = (StatusCode, &'static str, String);
async fn get_flags(catalogue: &Catalogue, id: &str) -> Result<Vec<Flag>, FlagError> {
    catalogue.launcher.look();
    let config = catalogue.launcher.get();
    let harness = config
        .harnesses
        .iter()
        .find(|h| h.id == id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                parvi_terminals_contract::UNKNOWN_HARNESS,
                "Unknown harness".into(),
            )
        })?;
    if harness.command.is_empty() {
        return Ok(vec![]);
    }
    let path = resolve(&harness.command).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            parvi_terminals_contract::HARNESS_NOT_FOUND,
            "Harness binary not found in the node PATH".into(),
        )
    })?;
    let modified = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map_err(internal)?;
    if let Some((stamp, flags)) = catalogue.cache.lock().await.get(&path)
        && *stamp == modified
    {
        return Ok(flags.clone());
    }
    let mut cmd = tokio::process::Command::new(&path);
    cmd.arg("--help")
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(internal)?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let (status, output) = tokio::time::timeout(Duration::from_secs(4), async {
        let mut output = Vec::new();
        (&mut stdout)
            .take(1_048_577)
            .read_to_end(&mut output)
            .await?;
        if output.len() > 1_048_576 {
            return Err(std::io::Error::other("Harness help exceeds 1 MiB"));
        }
        Ok::<_, std::io::Error>((child.wait().await?, output))
    })
    .await
    .map_err(|_| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            parvi_terminals_contract::HARNESS_HELP_TIMEOUT,
            "Harness help timed out".into(),
        )
    })?
    .map_err(internal)?;
    if !status.success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            parvi_terminals_contract::HARNESS_HELP_FAILED,
            "Harness help failed".into(),
        ));
    }
    let parsed = parse(&String::from_utf8_lossy(&output));
    catalogue
        .cache
        .lock()
        .await
        .insert(path, (modified, parsed.clone()));
    Ok(parsed)
}
fn internal(error: std::io::Error) -> FlagError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        parvi_terminals_contract::HARNESS_HELP_ERROR,
        error.to_string(),
    )
}
fn resolve(command: &str) -> Option<PathBuf> {
    let path = Path::new(command);
    let candidates: Vec<PathBuf> = if path.components().count() > 1 || path.is_absolute() {
        vec![path.to_owned()]
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|p| p.join(path))
            .collect()
    };
    candidates.into_iter().find_map(|p| {
        if p.is_file() {
            p.canonicalize().ok()
        } else {
            None
        }
    })
}

/// Parse both clap's indented descriptions and commander's aligned columns.
fn parse(help: &str) -> Vec<Flag> {
    let mut flags: Vec<Flag> = Vec::new();
    let mut options = false;
    for line in help.lines() {
        let trimmed = line.trim();
        if trimmed == "Options:" {
            options = true;
            continue;
        }
        if options && !line.starts_with(char::is_whitespace) && trimmed.ends_with(':') {
            break;
        }
        if !options || trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('-') && line.len() - line.trim_start().len() < 10 {
            let split = trimmed.find("  ").unwrap_or(trimmed.len());
            let (spec, description) = trimmed.split_at(split);
            let names: Vec<&str> = spec.split_whitespace().collect();
            let Some(long) = names.iter().find(|n| n.starts_with("--")) else {
                continue;
            };
            let name = long
                .trim_end_matches(',')
                .split('=')
                .next()
                .unwrap_or(long)
                .to_string();
            if flags.iter().any(|f| f.name == name) {
                continue;
            }
            let short = names
                .iter()
                .find(|n| n.starts_with('-') && !n.starts_with("--"))
                .map(|n| n.trim_end_matches(',').to_string());
            let value = names
                .iter()
                .find(|n| n.starts_with('<') || n.starts_with('['))
                .map(|s| s.to_string())
                .or_else(|| long.split_once('=').map(|(_, v)| v.to_string()));
            flags.push(Flag {
                name,
                short,
                value,
                description: description.trim().to_string(),
                values: vec![],
            });
        } else if let Some(flag) = flags.last_mut() {
            if !flag.description.is_empty() {
                flag.description.push(' ');
            }
            flag.description.push_str(trimmed);
        }
    }
    for flag in &mut flags {
        if let Some((_, rest)) = flag.description.split_once("[possible values:")
            && let Some((values, _)) = rest.split_once(']')
        {
            flag.values = values
                .split(',')
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect();
        }
        if let Some((_, rest)) = flag.description.split_once("(choices:")
            && let Some((values, _)) = rest.split_once(')')
        {
            flag.values = values
                .split(',')
                .map(|v| v.trim().trim_matches('"').to_string())
                .filter(|v| !v.is_empty())
                .collect();
        }
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_harness_help() {
        let claude = parse(include_str!("../tests/fixtures/launch/claude-help.txt"));
        let codex = parse(include_str!("../tests/fixtures/launch/codex-help.txt"));
        let get = |name: &str| claude.iter().find(|f| f.name == name).unwrap();
        assert_eq!(get("--resume").short.as_deref(), Some("-r"));
        assert_eq!(get("--print").short.as_deref(), Some("-p"));
        assert!(get("--print").description.contains("Print response"));
        assert_eq!(get("--output-format").value.as_deref(), Some("<format>"));
        assert_eq!(
            get("--output-format").values,
            ["text", "json", "stream-json"]
        );
        assert_eq!(get("--input-format").values, ["text", "stream-json"]);
        assert!(
            get("--permission-mode")
                .values
                .contains(&"acceptEdits".to_string())
        );
        assert!(
            codex
                .iter()
                .any(|f| f.description.contains("- on-request:"))
        );
        assert!(
            claude
                .iter()
                .any(|f| f.name == "--add-dir" && f.value.as_deref() == Some("<directories...>"))
        );
        assert!(codex.iter().any(|f| f.name == "--model"
            && f.short.as_deref() == Some("-m")
            && f.value.as_deref() == Some("<MODEL>")));
        assert!(
            codex
                .iter()
                .any(|f| f.name == "--sandbox" && !f.values.is_empty())
        );
    }
}
