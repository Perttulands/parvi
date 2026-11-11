//! Whose setup a node reports: a home folder and the folders it finds
//! programs in. A node reports its own; tests hand it a scratch one.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Home {
    pub dir: PathBuf,
    /// Where commands are looked for, in order.
    pub path: Vec<PathBuf>,
}

impl Home {
    /// This process's `HOME` and `PATH`, with `~/.local/bin` after `PATH`:
    /// the Claude Code, Codex and Pi installers put their commands there,
    /// and a systemd unit's `PATH` leaves it out.
    pub fn from_env() -> Option<Home> {
        let dir = PathBuf::from(std::env::var_os(if cfg!(windows) {
            "USERPROFILE"
        } else {
            "HOME"
        })?);
        let mut path: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let local = dir.join(".local").join("bin");
        if !path.contains(&local) {
            path.push(local);
        }
        Some(Home { dir, path })
    }

    /// The executable `command` resolves to, if any.
    pub fn which(&self, command: &str) -> Option<PathBuf> {
        let names: Vec<String> = if cfg!(windows) {
            ["exe", "cmd", "bat"]
                .iter()
                .map(|ext| format!("{command}.{ext}"))
                .collect()
        } else {
            vec![command.to_owned()]
        };
        self.path
            .iter()
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .find(|candidate| is_executable(candidate))
    }

    /// `PATH` as a child process should get it.
    pub fn path_var(&self) -> OsString {
        std::env::join_paths(&self.path).unwrap_or_default()
    }

    /// A path as the operator reads it: `~/…` under the home folder.
    pub fn show(&self, path: &Path) -> String {
        match path.strip_prefix(&self.dir) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
            Ok(rest) => format!("~/{}", rest.to_string_lossy().replace('\\', "/")),
            Err(_) => path.to_string_lossy().into_owned(),
        }
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_paths_under_home_with_a_tilde() {
        let home = Home {
            dir: "/home/p".into(),
            path: vec![],
        };
        assert_eq!(home.show(Path::new("/home/p")), "~");
        assert_eq!(
            home.show(Path::new("/home/p/.claude/skills")),
            "~/.claude/skills"
        );
        assert_eq!(home.show(Path::new("/srv/agent-kit")), "/srv/agent-kit");
    }
}
