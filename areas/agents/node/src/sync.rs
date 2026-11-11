//! Sync: `git pull` and the checkout's setup script, typed into a new tmux
//! session so the operator can watch it and read what it said. Git is kept
//! non-interactive even though the session has a terminal.

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
    #[cfg(windows)]
    let prefix = "$env:GIT_TERMINAL_PROMPT = '0'; git pull --ff-only";
    #[cfg(not(windows))]
    let prefix = "export GIT_TERMINAL_PROMPT=0; git pull --ff-only";
    match setup {
        #[cfg(windows)]
        Some(script) => format!("{prefix}; if ($?) {{ .\\{script} }}"),
        #[cfg(not(windows))]
        Some(script) => format!("{prefix} && ./{script}"),
        None => prefix.into(),
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

    #[cfg(unix)]
    #[test]
    fn sync_is_noninteractive_and_runs_setup_only_after_a_successful_pull() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("parvi-sync-command-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let git = dir.join("git");
        let setup = dir.join("install.sh");
        std::fs::write(&git, "#!/bin/sh\nprintf '%s' \"$GIT_TERMINAL_PROMPT\" > git-prompt\nprintf '%s' \"$*\" > git-args\nif [ -f fail ]; then echo 'fatal: credentials unavailable' >&2; exit 1; fi\n").unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            &setup,
            "#!/bin/sh\nprintf '%s' \"$GIT_TERMINAL_PROMPT\" > setup-prompt\n",
        )
        .unwrap();
        std::fs::set_permissions(&setup, std::fs::Permissions::from_mode(0o755)).unwrap();
        let invoke = || {
            Command::new("/bin/sh")
                .arg("-c")
                .arg(line(Some("install.sh")))
                .current_dir(&dir)
                .env("PATH", &dir)
                .env("GIT_TERMINAL_PROMPT", "1")
                .output()
                .unwrap()
        };
        std::fs::write(dir.join("fail"), "").unwrap();
        let failed = invoke();
        assert!(!failed.status.success());
        assert!(String::from_utf8_lossy(&failed.stderr).contains("fatal: credentials unavailable"));
        assert_eq!(
            std::fs::read_to_string(dir.join("git-prompt")).unwrap(),
            "0"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("git-args")).unwrap(),
            "pull --ff-only"
        );
        assert!(!dir.join("setup-prompt").exists());
        std::fs::remove_file(dir.join("fail")).unwrap();
        assert!(invoke().status.success());
        assert_eq!(
            std::fs::read_to_string(dir.join("setup-prompt")).unwrap(),
            "0"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
