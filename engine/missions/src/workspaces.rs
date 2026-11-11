//! Git worktrees owned by mission runs. Git is always invoked directly.
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug)]
pub struct Worktree {
    pub path: PathBuf,
    pub checkout: PathBuf,
    pub branch: String,
    pub base: String,
}

pub struct Workspaces {
    root: PathBuf,
    repositories: BTreeMap<String, PathBuf>,
}
impl Workspaces {
    pub fn new(root: impl Into<PathBuf>, repositories: BTreeMap<String, PathBuf>) -> Self {
        Self {
            root: root.into(),
            repositories,
        }
    }
    pub fn checkout(&self, repository: &str) -> Option<&Path> {
        self.repositories.get(repository).map(PathBuf::as_path)
    }
    pub fn prepare(&self, run: &str, repository: &str, base: &str) -> io::Result<Worktree> {
        if !run.starts_with("run_") || !run.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid run id",
            ));
        }
        let checkout = self
            .checkout(repository)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no checkout"))?
            .to_path_buf();
        let branch = format!("parvi/run-{run}");
        let base = if base.is_empty() {
            git(&checkout, ["symbolic-ref", "--quiet", "--short", "HEAD"])?
        } else {
            base.to_owned()
        };
        // Resolve as a commit, and reject option-like values.
        if base.starts_with('-') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid base"));
        }
        git(
            &checkout,
            [
                "rev-parse",
                "--verify",
                &format!("refs/heads/{base}^{{commit}}"),
            ],
        )?;
        fs::create_dir_all(&self.root)?;
        let path = self.root.join(run);
        reserve(&path)?;
        if let Err(error) = git(
            &checkout,
            ["branch", &branch, &format!("refs/heads/{base}")],
        ) {
            fs::remove_dir(&path)?;
            return Err(error);
        }
        let added = git(
            &checkout,
            [
                OsStr::new("worktree"),
                OsStr::new("add"),
                path.as_os_str(),
                OsStr::new(&branch),
            ],
        );
        if let Err(error) = added {
            let cleanup = cleanup_failed_add(&checkout, &path)
                .and_then(|()| git(&checkout, ["branch", "-D", &branch]).map(|_| ()));
            return Err(with_cleanup(error, cleanup));
        }
        Ok(Worktree {
            path,
            checkout,
            branch,
            base,
        })
    }
    pub fn commit_turn(
        &self,
        tree: &Worktree,
        run: &str,
        node: &str,
        attempt: i64,
    ) -> io::Result<Option<String>> {
        git(&tree.path, ["add", "-A"])?;
        let output = Command::new("git")
            .current_dir(&tree.path)
            .args(["diff", "--cached", "--quiet", "--exit-code"])
            .output()?;
        match output.status.code() {
            Some(0) => return Ok(None),
            Some(1) => (),
            _ => {
                return Err(io::Error::other(
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                ));
            }
        }
        git(
            &tree.path,
            [
                "commit",
                "-m",
                &format!("Run {run} step {node} attempt {attempt}"),
            ],
        )?;
        Ok(Some(git(&tree.path, ["rev-parse", "HEAD"])?))
    }
    /// Removes the run's worktree and branch, and the refs the run shared on
    /// origin: its branch, and each turn ref in `published` (reference to the
    /// commit its node pushed). A shared ref is deleted only while it still
    /// points where Parvi pushed it; one that moved or that Parvi has no
    /// record of is left, and said in the notes returned.
    pub fn remove(
        &self,
        tree: &Worktree,
        force: bool,
        published: &BTreeMap<String, String>,
    ) -> io::Result<Vec<String>> {
        if force {
            let notes = remove_shared_refs(tree, published)?;
            if tree.path.exists() {
                git(
                    &tree.checkout,
                    [
                        OsStr::new("worktree"),
                        OsStr::new("remove"),
                        OsStr::new("--force"),
                        tree.path.as_os_str(),
                    ],
                )?;
            }
            git(&tree.checkout, ["branch", "-D", &tree.branch])?;
            return Ok(notes);
        }
        if tree.path.exists() {
            let output = Command::new("git")
                .current_dir(&tree.path)
                .args([
                    "status",
                    "--porcelain=v1",
                    "--ignored",
                    "--untracked-files=all",
                    "-z",
                ])
                .output()?;
            if !output.status.success() {
                return Err(io::Error::other(format!(
                    "git status: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            if !output.stdout.is_empty() {
                return Err(io::Error::other(
                    "worktree contains uncommitted or ignored files",
                ));
            }
        }
        let base_ref = format!("refs/heads/{}", tree.base);
        git(
            &tree.checkout,
            ["merge-base", "--is-ancestor", &tree.branch, &base_ref],
        )?;
        let notes = remove_shared_refs(tree, published)?;
        // Branch deletion checks HEAD. A detached temporary worktree puts HEAD on
        // the recorded base without touching the operator's checkout.
        let check = self
            .root
            .join(format!(".land-{}", tree.branch.replace('/', "-")));
        reserve(&check)?;
        if let Err(error) = git(
            &tree.checkout,
            [
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--detach"),
                check.as_os_str(),
                OsStr::new(&base_ref),
            ],
        ) {
            return Err(with_cleanup(
                error,
                cleanup_failed_add(&tree.checkout, &check),
            ));
        }
        let result = (|| {
            if tree.path.exists() {
                git(
                    &tree.checkout,
                    [
                        OsStr::new("worktree"),
                        OsStr::new("remove"),
                        OsStr::new("--"),
                        tree.path.as_os_str(),
                    ],
                )?;
            }
            let remote = format!("branch.{}.remote=.", tree.branch);
            let merge = format!("branch.{}.merge={base_ref}", tree.branch);
            git(
                &check,
                ["-c", &remote, "-c", &merge, "branch", "-d", &tree.branch],
            )?;
            Ok(())
        })();
        let cleanup = git(
            &tree.checkout,
            [
                OsStr::new("worktree"),
                OsStr::new("remove"),
                OsStr::new("--"),
                check.as_os_str(),
            ],
        );
        result.and(cleanup.map(|_| ())).map(|()| notes)
    }
}
/// A run with steps on other environments pushed its branch and their turns
/// to the checkout's `origin` (`run_workspace::prepare_placed_turn`); landing
/// or removing the run deletes them there too, each with a lease on the
/// object Parvi pushed, so nobody's later push is lost. A run that never
/// pushed has no remote-tracking branch and touches no remote.
fn remove_shared_refs(
    tree: &Worktree,
    published: &BTreeMap<String, String>,
) -> io::Result<Vec<String>> {
    let tracking = format!("refs/remotes/origin/{}", tree.branch);
    // Pushing the run branch set its remote-tracking ref: where Parvi last
    // pushed it. Without one, this run shared nothing.
    let Ok(pushed) = git(
        &tree.checkout,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{tracking}^{{commit}}"),
        ],
    ) else {
        return Ok(Vec::new());
    };
    let run = tree.branch.trim_start_matches("parvi/run-");
    let turns = format!("refs/parvi/turns/{run}/*");
    let branch = format!("refs/heads/{}", tree.branch);
    let mut expected = published.clone();
    expected.insert(branch.clone(), pushed);
    let listed = git(&tree.checkout, ["ls-remote", "origin", &branch, &turns])?;
    let mut leases = Vec::new();
    let mut notes = Vec::new();
    let mut branch_left = false;
    for (oid, reference) in listed.lines().filter_map(|line| line.split_once('\t')) {
        match expected.get(reference) {
            Some(want) if want == oid => leases.push((reference.to_owned(), oid.to_owned())),
            other => {
                branch_left |= reference == branch;
                notes.push(match other {
                    Some(want) => format!(
                        "left {reference} on origin: it moved to {oid} since Parvi pushed {want}"
                    ),
                    None => {
                        format!("left {reference} on origin: Parvi has no record of pushing it")
                    }
                });
            }
        }
    }
    if !leases.is_empty() {
        // The lease makes the remote refuse a deletion if the ref moved
        // after it was listed; that refusal fails the removal.
        let mut args = vec!["push".to_owned(), "--quiet".into()];
        for (reference, oid) in &leases {
            args.push(format!("--force-with-lease={reference}:{oid}"));
        }
        args.push("origin".into());
        args.extend(leases.iter().map(|(reference, _)| format!(":{reference}")));
        git(&tree.checkout, &args)?;
    }
    if !branch_left {
        // The push removed the remote-tracking branch with it on current
        // git; make sure, so a later removal does not try the remote again.
        let _ = Command::new("git")
            .current_dir(&tree.checkout)
            .args(["update-ref", "-d", &tracking])
            .output();
    }
    Ok(notes)
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

// Reserve the path ourselves so cleanup never removes a pre-existing directory.
fn reserve(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

// A failed post-checkout hook can leave a fully registered worktree. Remove it
// through Git first; only remove our reserved directory if Git did not register it.
fn cleanup_failed_add(checkout: &Path, path: &Path) -> io::Result<()> {
    let removed = git(
        checkout,
        [
            OsStr::new("worktree"),
            OsStr::new("remove"),
            OsStr::new("--force"),
            path.as_os_str(),
        ],
    );
    if removed.is_err() {
        let output = Command::new("git")
            .current_dir(checkout)
            .args(["worktree", "list", "--porcelain", "-z"])
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(
                "cannot inspect worktree registrations after failed add",
            ));
        }
        let registered = output
            .stdout
            .split(|b| *b == 0)
            .filter_map(|entry| entry.strip_prefix(b"worktree "))
            .any(|entry| Path::new(&String::from_utf8_lossy(entry).into_owned()) == path);
        if registered {
            return removed.map(|_| ());
        }
    }
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}
fn with_cleanup(error: io::Error, cleanup: io::Result<()>) -> io::Error {
    match cleanup {
        Ok(()) => error,
        Err(cleanup) => io::Error::other(format!("{error}; cleanup failed: {cleanup}")),
    }
}
