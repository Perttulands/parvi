//! A skills repository's checkout, such as agent-kit: where it is, what it
//! has checked out, how far it is from its upstream, and the setup script
//! a sync runs after fast-forwarding.

use std::path::{Path, PathBuf};

use super::{Home, run};

/// The setup scripts a sync looks for at the top of a checkout, in order.
#[cfg(not(windows))]
pub const SETUP: &[&str] = &["install.sh", "setup.sh"];
#[cfg(windows)]
pub const SETUP: &[&str] = &["install.ps1", "setup.ps1"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub path: PathBuf,
    pub branch: Option<String>,
    /// The short commit id checked out.
    pub commit: Option<String>,
    pub subject: Option<String>,
    /// Like `origin/main`; `None` when the branch tracks nothing.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Changed or untracked files.
    pub dirty: bool,
    pub setup: Option<&'static str>,
    pub fetch: Fetch,
}

/// How current `behind` and `ahead` are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetch {
    /// Local refs as last updated by any external or prior fetch.
    Local,
    /// As of the checkout's last fetch; a fresh one is under way.
    Fetching,
    Fetched,
    Failed(String),
}

impl Checkout {
    /// The folder's name, as the operator calls the repository.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }
}

/// The git checkout holding `path`, if any: the nearest folder up from it
/// with a `.git`.
pub fn holding(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_owned)
}

/// Reads the checkout's state from git, as of its last fetch.
pub async fn read(home: &Home, path: &Path, fetch: Fetch) -> Checkout {
    let git = |args: &'static [&'static str]| run::git(home, path, args);
    let (branch, commit, subject, upstream, status) = tokio::join!(
        git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        git(&["rev-parse", "--short", "HEAD"]),
        git(&["log", "-1", "--format=%s"]),
        git(&[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}"
        ]),
        git(&["status", "--porcelain"]),
    );
    let (ahead, behind) = match upstream {
        Some(_) => git(&["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
            .await
            .and_then(|counts| parse_counts(&counts))
            .unwrap_or((0, 0)),
        None => (0, 0),
    };
    Checkout {
        path: path.to_owned(),
        branch: branch.filter(|b| b != "HEAD"),
        commit,
        subject,
        upstream,
        ahead,
        behind,
        dirty: status.is_some_and(|s| !s.is_empty()),
        setup: SETUP.iter().copied().find(|name| path.join(name).is_file()),
        fetch,
    }
}

/// How many upstream commits not checked out touch `relative`.
pub async fn behind_in(home: &Home, checkout: &Path, relative: &Path) -> u32 {
    let relative = relative.to_string_lossy();
    let count = run::git(
        home,
        checkout,
        &["rev-list", "--count", "HEAD..@{upstream}", "--", &relative],
    )
    .await;
    count.and_then(|c| c.parse().ok()).unwrap_or(0)
}

/// `ahead<TAB>behind`, as `rev-list --left-right --count` prints them.
fn parse_counts(counts: &str) -> Option<(u32, u32)> {
    let (ahead, behind) = counts.split_once(char::is_whitespace)?;
    Some((ahead.trim().parse().ok()?, behind.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ahead_and_behind() {
        assert_eq!(parse_counts("1\t3"), Some((1, 3)));
        assert_eq!(parse_counts("0\t0"), Some((0, 0)));
        assert_eq!(parse_counts("x"), None);
    }
}
