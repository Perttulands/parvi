//! Isolated turn worktrees and their durable publication to the run branch.
use crate::coordinator::Control;
use crate::run_worker::{append_live, event};
use crate::{Coordinator, RunEvent, Workspaces, Worktree};
use parvi_mission_model::RunStart;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) fn turn_path(c: &Coordinator, id: &str, node: &str, attempt: i64) -> PathBuf {
    c.inner.root.join("worktrees").join(format!(
        ".turn-{id}-{:x}-{attempt}",
        Sha256::digest(node.as_bytes())
    ))
}

pub(crate) fn prepare_turn(
    c: &Coordinator,
    id: &str,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
    node: &str,
    attempt: i64,
) -> io::Result<PathBuf> {
    if req.repository.is_empty() {
        return Ok(workspace.to_path_buf());
    }
    let _guard = control.workspace.lock().unwrap();
    if control.canceled.load(std::sync::atomic::Ordering::Acquire) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
    }
    let path = turn_path(c, id, node, attempt);
    let base = git(workspace, &["rev-parse".as_ref(), "HEAD".as_ref()])?;
    append_workspace(
        c,
        id,
        control,
        event("workspace_started", node, attempt, json!({"base":base})),
    )?;
    git(
        workspace,
        &[
            "worktree".as_ref(),
            "add".as_ref(),
            "--detach".as_ref(),
            path.as_os_str(),
            base.as_ref(),
        ],
    )?;
    Ok(path)
}

/// A turn on another environment starts from the run branch's head, pushed
/// to the repository's shared remote for that environment to fetch.
pub(crate) fn prepare_placed_turn(
    c: &Coordinator,
    id: &str,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
    (node, place): (&str, &str),
    attempt: i64,
) -> io::Result<crate::TurnWorkspace> {
    if req.repository.is_empty() {
        return Err(io::Error::other(format!(
            "step {node} runs on {place}, which needs a repository run to share its workspace"
        )));
    }
    let _guard = control.workspace.lock().unwrap();
    if control.canceled.load(std::sync::atomic::Ordering::Acquire) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
    }
    let base = git(workspace, &["rev-parse".as_ref(), "HEAD".as_ref()])?;
    append_workspace(
        c,
        id,
        control,
        event(
            "workspace_started",
            node,
            attempt,
            json!({"base":base,"place":place}),
        ),
    )?;
    let branch = format!("parvi/run-{id}");
    let target = format!("HEAD:refs/heads/{branch}");
    git(
        workspace,
        &[
            "push".as_ref(),
            "--quiet".as_ref(),
            "origin".as_ref(),
            target.as_ref(),
        ],
    )?;
    append_workspace(
        c,
        id,
        control,
        event(
            "workspace_push",
            node,
            attempt,
            json!({"commit":base,"branch":branch}),
        ),
    )?;
    Ok(crate::TurnWorkspace {
        run_id: id.into(),
        node_id: node.into(),
        attempt,
        repository: req.repository.clone(),
        commit: base,
    })
}

/// The commits a placed turn published: fetched from the shared remote,
/// from the turn's base to its published commit.
fn placed_commits(
    workspace: &Path,
    node: &str,
    attempt: i64,
    base: &str,
    events: &[RunEvent],
) -> io::Result<Vec<String>> {
    let published = events
        .iter()
        .find(|e| e.kind == "workspace_turn" && e.node_id == node && e.attempt == attempt)
        .ok_or_else(|| {
            io::Error::other(format!(
                "step {node} attempt {attempt} ran on another environment and did not publish its work"
            ))
        })?;
    let field = |name: &str| {
        published
            .data
            .get(name)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned()
    };
    let (commit, reference) = (field("commit"), field("reference"));
    git(
        workspace,
        &[
            "fetch".as_ref(),
            "--quiet".as_ref(),
            "origin".as_ref(),
            reference.as_ref(),
        ],
    )?;
    let range = format!("{base}..{commit}");
    Ok(git(
        workspace,
        &[
            "rev-list".as_ref(),
            "--reverse".as_ref(),
            "--topo-order".as_ref(),
            range.as_ref(),
        ],
    )?
    .lines()
    .map(str::to_owned)
    .collect())
}

/// Save the output before publication, but do not expose it to routing until
/// its commits and worktree removal are durable.
pub(crate) fn publish_output(
    c: &Coordinator,
    id: &str,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
    mut output: RunEvent,
) -> io::Result<()> {
    if req.repository.is_empty() {
        if append_live(c, id, control, output)? {
            return Ok(());
        }
        return Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
    }
    output.kind = "node_output_pending".into();
    if !append_live(c, id, control, output.clone())? {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
    }
    commit_turn(
        c,
        id,
        req,
        workspace,
        control,
        &output.node_id,
        output.attempt,
    )?;
    output.kind = "node_output".into();
    if !append_live(c, id, control, output)? {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
    }
    Ok(())
}

pub(crate) fn commit_turn(
    c: &Coordinator,
    id: &str,
    req: &RunStart,
    workspace: &Path,
    control: &Control,
    node: &str,
    attempt: i64,
) -> io::Result<()> {
    if req.repository.is_empty() {
        return Ok(());
    }
    let _guard = control.workspace.lock().unwrap();
    if control.canceled.load(std::sync::atomic::Ordering::Acquire) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"));
    }
    let events = c.ledger(id)?.read().map_err(crate::error::to_io)?;
    let turn = |kind: &str| {
        events
            .iter()
            .find(|e| e.kind == kind && e.node_id == node && e.attempt == attempt)
    };
    let path = turn_path(c, id, node, attempt);
    if turn("workspace_settled").is_some() {
        return remove(workspace, &path);
    }
    let first = &events[0];
    let field = |name: &str| {
        first
            .data
            .get(name)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned()
    };
    let tree = Worktree {
        path: path.clone(),
        checkout: PathBuf::from(field("checkout")),
        branch: format!("parvi/run-{id}"),
        base: field("base"),
    };
    let workspaces = Workspaces::new(
        c.inner.root.join("worktrees"),
        c.inner.settings.repositories.clone(),
    );
    let mut published = turn("workspace_published")
        .and_then(|e| e.data.get("commit"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    if published.is_none() {
        let commits: Vec<String> = if let Some(snapshot) = turn("workspace_snapshot") {
            serde_json::from_value(snapshot.data.get("commits").cloned().unwrap_or_default())
                .map_err(io::Error::other)?
        } else if turn("workspace_push").is_some() {
            let base = turn("workspace_started")
                .and_then(|e| e.data.get("base"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| io::Error::other("turn base is missing"))?;
            let commits = placed_commits(workspace, node, attempt, base, &events)?;
            append_workspace(
                c,
                id,
                control,
                event(
                    "workspace_snapshot",
                    node,
                    attempt,
                    json!({"base":base,"commits":commits}),
                ),
            )?;
            commits
        } else {
            let base = turn("workspace_started")
                .and_then(|e| e.data.get("base"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| io::Error::other("turn base is missing"))?;
            if !path.exists() {
                return Err(io::Error::other(format!(
                    "workspace for {node} attempt {attempt} is missing"
                )));
            }
            workspaces
                .commit_turn(&tree, id, node, attempt)
                .map_err(crate::error::to_io)?;
            let range = format!("{base}..HEAD");
            let commits = git(
                &path,
                &[
                    "rev-list".as_ref(),
                    "--reverse".as_ref(),
                    "--topo-order".as_ref(),
                    range.as_ref(),
                ],
            )?
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
            append_workspace(
                c,
                id,
                control,
                event(
                    "workspace_snapshot",
                    node,
                    attempt,
                    json!({"base":base,"commits":commits}),
                ),
            )?;
            commits
        };
        for source in commits {
            let prior = events.iter().any(|e| {
                e.kind == "workspace_commit"
                    && e.node_id == node
                    && e.attempt == attempt
                    && e.data.get("source").and_then(|v| v.as_str()) == Some(&source)
            });
            if prior {
                continue;
            }
            let history = git(workspace, &["log".as_ref(), "--format=%B".as_ref()])?;
            let marker = format!("(cherry picked from commit {source})");
            if !history.lines().any(|line| line == marker)
                && let Err(error) = git(
                    workspace,
                    &[
                        "cherry-pick".as_ref(),
                        "-x".as_ref(),
                        "--allow-empty".as_ref(),
                        "--keep-redundant-commits".as_ref(),
                        source.as_ref(),
                    ],
                )
            {
                let _ = git(workspace, &["cherry-pick".as_ref(), "--abort".as_ref()]);
                return Err(error);
            }
            let head = git(workspace, &["rev-parse".as_ref(), "HEAD".as_ref()])?;
            append_workspace(
                c,
                id,
                control,
                event(
                    "workspace_commit",
                    node,
                    attempt,
                    json!({"commit":head,"source":source}),
                ),
            )?;
        }
        let head = git(workspace, &["rev-parse".as_ref(), "HEAD".as_ref()])?;
        append_workspace(
            c,
            id,
            control,
            event("workspace_published", node, attempt, json!({"commit":head})),
        )?;
        published = Some(head);
    }
    remove(workspace, &path)?;
    append_workspace(
        c,
        id,
        control,
        event(
            "workspace_settled",
            node,
            attempt,
            json!({"commit":published}),
        ),
    )?;
    Ok(())
}

fn append_workspace(
    c: &Coordinator,
    id: &str,
    control: &Control,
    event: RunEvent,
) -> io::Result<()> {
    if append_live(c, id, control, event)? {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Interrupted, "run canceled"))
    }
}

fn git(cwd: &Path, args: &[&OsStr]) -> io::Result<String> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
pub(crate) fn remove(workspace: &Path, path: &Path) -> io::Result<()> {
    let registered = if path.exists() {
        true
    } else {
        let entries = git(
            workspace,
            &[
                "worktree".as_ref(),
                "list".as_ref(),
                "--porcelain".as_ref(),
                "-z".as_ref(),
            ],
        )?;
        entries
            .split('\0')
            .filter_map(|entry| entry.strip_prefix("worktree "))
            .any(|entry| Path::new(entry) == path)
    };
    if registered {
        git(
            workspace,
            &[
                "worktree".as_ref(),
                "remove".as_ref(),
                "--force".as_ref(),
                path.as_os_str(),
            ],
        )?;
    }
    Ok(())
}
