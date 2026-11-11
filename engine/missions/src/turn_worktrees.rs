//! Turn worktrees on a seat's own node, for a step placed on another
//! environment than its coordinator's. The coordinator pushes the run branch
//! to the repository's shared remote (the checkout's `origin`); the seat's
//! node fetches it and starts the turn from the exact commit the coordinator
//! named, then publishes the turn's commit as a ref of its own that the
//! coordinator fetches and serializes onto the run branch.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

/// The turn a coordinator asks another node to prepare or publish.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnWorkspace {
    pub run_id: String,
    /// The step (formation) the turn belongs to.
    pub node_id: String,
    pub attempt: i64,
    /// The repository, as named in `missions.repositories`.
    pub repository: String,
    /// The commit the turn starts from, on the run branch.
    pub commit: String,
}

/// A published turn: its last commit, and the remote ref that holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedTurn {
    pub commit: String,
    pub reference: String,
}

impl TurnWorkspace {
    /// The run branch every node shares through the remote.
    pub fn branch(&self) -> String {
        format!("parvi/run-{}", self.run_id)
    }
    /// The remote ref a published turn is pushed to.
    pub fn reference(&self) -> String {
        format!(
            "refs/parvi/turns/{}/{}-{}",
            self.run_id,
            &step_hash(&self.node_id)[..12],
            self.attempt
        )
    }
    fn check(&self) -> io::Result<()> {
        let run_ok = self.run_id.starts_with("run_")
            && self
                .run_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_');
        let commit_ok = matches!(self.commit.len(), 40 | 64)
            && self.commit.bytes().all(|b| b.is_ascii_hexdigit());
        if !run_ok || !commit_ok || self.attempt < 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid turn workspace",
            ));
        }
        Ok(())
    }
}

/// This node's turn worktrees, made from its own checkouts.
#[derive(Clone, Debug)]
pub struct TurnWorktrees {
    root: PathBuf,
    repositories: BTreeMap<String, PathBuf>,
    /// One lock per turn: a retried call (after its HTTP request timed out)
    /// waits for the first instead of running git beside it.
    busy: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}

impl TurnWorktrees {
    pub fn new(root: impl Into<PathBuf>, repositories: BTreeMap<String, PathBuf>) -> Self {
        Self {
            root: root.into(),
            repositories,
            busy: Arc::default(),
        }
    }

    pub fn with_repositories(self, repositories: BTreeMap<String, PathBuf>) -> Self {
        Self {
            repositories,
            ..self
        }
    }

    fn path(&self, turn: &TurnWorkspace) -> PathBuf {
        self.root.join(&turn.run_id).join(format!(
            "turn-{}-{}",
            &step_hash(&turn.node_id)[..12],
            turn.attempt
        ))
    }

    fn lock(&self, turn: &TurnWorkspace) -> Arc<Mutex<()>> {
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        busy.entry(self.path(turn)).or_default().clone()
    }

    fn published_path(&self, turn: &TurnWorkspace) -> PathBuf {
        self.path(turn).with_extension("published")
    }

    /// The turn's worktree, detached at the named commit after fetching the
    /// run branch. Preparing again returns the same worktree.
    pub fn prepare(&self, turn: &TurnWorkspace) -> Result<PathBuf, crate::Error> {
        (|| -> io::Result<PathBuf> {
            turn.check()?;
            let lock = self.lock(turn);
            let _turn = lock.lock().unwrap_or_else(|e| e.into_inner());
            let path = self.path(turn);
            if self.published_path(turn).exists() {
                return Err(io::Error::other("turn workspace is already published"));
            }
            if path.exists() {
                return Ok(path);
            }
            let checkout = self.repositories.get(&turn.repository).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no_checkout: no checkout of {} here", turn.repository),
                )
            })?;
            git(
                checkout,
                [
                    "fetch",
                    "--quiet",
                    "origin",
                    &format!("refs/heads/{}", turn.branch()),
                ],
            )?;
            git(
                checkout,
                ["cat-file", "-e", &format!("{}^{{commit}}", turn.commit)],
            )?;
            fs::create_dir_all(path.parent().unwrap_or(&self.root))?;
            git(
                checkout,
                [
                    OsStr::new("worktree"),
                    OsStr::new("add"),
                    OsStr::new("--detach"),
                    path.as_os_str(),
                    OsStr::new(&turn.commit),
                ],
            )?;
            Ok(path)
        })()
        .map_err(crate::error::io)
    }

    /// Commits what the turn changed, pushes it to the turn's ref and
    /// removes the worktree. Publishing again returns the first answer, and
    /// removes the worktree if an earlier call stopped before it could.
    pub fn publish(&self, turn: &TurnWorkspace) -> Result<PublishedTurn, crate::Error> {
        (|| -> io::Result<PublishedTurn> {
            turn.check()?;
            let lock = self.lock(turn);
            let _turn = lock.lock().unwrap_or_else(|e| e.into_inner());
            let marker = self.published_path(turn);
            let path = self.path(turn);
            if let Ok(text) = fs::read_to_string(&marker) {
                let published = serde_json::from_str(&text).map_err(io::Error::other)?;
                self.remove(turn, &path)?;
                return Ok(published);
            }
            if !path.exists() {
                return Err(io::Error::other("turn workspace is missing"));
            }
            git(&path, ["add", "-A"])?;
            let staged = Command::new("git")
                .current_dir(&path)
                .args(["diff", "--cached", "--quiet", "--exit-code"])
                .status()?;
            if !staged.success() {
                git(
                    &path,
                    [
                        "commit",
                        "--quiet",
                        "-m",
                        &format!(
                            "Run {} step {} attempt {}",
                            turn.run_id, turn.node_id, turn.attempt
                        ),
                    ],
                )?;
            }
            let published = PublishedTurn {
                commit: git(&path, ["rev-parse", "HEAD"])?,
                reference: turn.reference(),
            };
            git(
                &path,
                [
                    "push",
                    "--quiet",
                    "--force",
                    "origin",
                    &format!("HEAD:{}", published.reference),
                ],
            )?;
            let text = serde_json::to_string(&published).map_err(io::Error::other)?;
            crate::write::atomic(&marker, text.as_bytes())?;
            self.remove(turn, &path)?;
            Ok(published)
        })()
        .map_err(crate::error::io)
    }

    fn remove(&self, turn: &TurnWorkspace, path: &Path) -> io::Result<()> {
        if !path.exists() {
            return Ok(());
        }
        let checkout = self
            .repositories
            .get(&turn.repository)
            .ok_or_else(|| io::Error::other("no checkout"))?;
        git(
            checkout,
            [
                OsStr::new("worktree"),
                OsStr::new("remove"),
                OsStr::new("--force"),
                path.as_os_str(),
            ],
        )?;
        Ok(())
    }
}

fn step_hash(step: &str) -> String {
    hex::encode(Sha256::digest(step.as_bytes()))
}

fn git<I, S>(cwd: &Path, args: I) -> io::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
