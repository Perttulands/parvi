//! Sync: `git pull` and the checkout's setup script, typed into a new tmux
//! session so the operator watches it run, answers it if it asks, and reads
//! what it said.

use anyhow::{Context, Result};

use super::checkout::SETUP;
use super::report::checkout_paths;
use super::{Home, skills};
use parvi_term::{Sessions, Start};

/// Starts syncing the checkout the operator knows as `shown` (as the report
/// shows its path), if it is one the skills come from. Returns the new
/// session's name.
pub async fn sync(home: &Home, sessions: &dyn Sessions, shown: &str) -> Result<String> {
    let path = checkout_paths(&skills::scan(home))
        .into_iter()
        .find(|path| home.show(path) == shown)
        .with_context(|| format!("{shown} is not a checkout the skills come from"))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let taken: Vec<String> = sessions
        .list()
        .into_iter()
        .filter(|s| s.user.is_none())
        .map(|s| s.name)
        .collect();
    let session = free_name(&format!("sync-{}", tmux_safe(&name)), &taken);
    let setup = SETUP
        .iter()
        .copied()
        .find(|script| path.join(script).is_file());
    sessions
        .start(Start {
            name: session.clone(),
            folder: path.to_string_lossy().into_owned(),
            command: line(setup),
            user: String::new(),
        })
        .await?;
    Ok(session)
}

/// What the session types: the pull, then the setup if the pull worked.
fn line(setup: Option<&str>) -> String {
    match setup {
        #[cfg(windows)]
        Some(script) => format!("git pull --ff-only; if ($?) {{ .\\{script} }}"),
        #[cfg(not(windows))]
        Some(script) => format!("git pull --ff-only && ./{script}"),
        None => "git pull --ff-only".into(),
    }
}

/// Letters, digits, `_` and `-`, as a session name allows.
fn tmux_safe(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    if safe.is_empty() {
        "checkout".into()
    } else {
        safe
    }
}

/// `base`, or `base-2`, `base-3`… if that is taken.
fn free_name(base: &str, taken: &[String]) -> String {
    (1..)
        .map(|n| {
            if n == 1 {
                base.to_owned()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|name| !taken.contains(name))
        .expect("a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_session_after_the_checkout() {
        assert_eq!(tmux_safe("agent-kit"), "agent-kit");
        assert_eq!(tmux_safe("my.kit"), "my-kit");
        let taken = vec!["sync-agent-kit".to_owned(), "sync-agent-kit-2".to_owned()];
        assert_eq!(free_name("sync-agent-kit", &taken), "sync-agent-kit-3");
        assert_eq!(free_name("sync-other", &taken), "sync-other");
    }

    #[cfg(not(windows))]
    #[test]
    fn pulls_then_runs_the_setup() {
        assert_eq!(
            line(Some("install.sh")),
            "git pull --ff-only && ./install.sh"
        );
        assert_eq!(line(None), "git pull --ff-only");
    }
}
