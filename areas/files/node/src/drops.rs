//! Drops: files attached to a Send land on the node of the session they are
//! sent to, so the agent there can read them by path (CHROTE's
//! `tmux_drops.go`, S27). Each file gets a folder of its own under the drops
//! folder, named by a random id; the folder stays private to the service user
//! until the file is whole, and then, for a session on another Unix user's
//! tmux server, that user alone is granted read (`setfacl`). The answer is
//! the path the Send puts in its message. See docs/protocol.md, Drops.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
/// The most one attachment carries.
pub const MAX_BYTES: usize = 256 << 20;

/// Where this node writes drops, and the users whose sessions it follows.
pub struct Drops {
    dir: PathBuf,
    users: Vec<String>,
}

impl Drops {
    pub fn new(dir: PathBuf, users: Vec<String>) -> Drops {
        Drops { dir, users }
    }

    /// Writes `bytes` as `name` in a new drop readable by `user` (empty is
    /// the service user), and returns its path.
    pub fn write(&self, name: &str, user: &str, bytes: &[u8]) -> Result<PathBuf> {
        if !user.is_empty() && !self.users.iter().any(|u| u == user) {
            bail!("this node follows no tmux server of user {user}");
        }
        root(&self.dir)?;
        let drop = self.dir.join(drop_id());
        create_private_dir(&drop)?;
        let written = (|| {
            let path = drop.join(clean_name(name));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options
                .open(&path)
                .with_context(|| format!("creating {}", path.display()))?;
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .with_context(|| format!("writing {}", path.display()))?;
            if !user.is_empty() {
                grant(&drop, user)?;
            }
            Ok(path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_dir_all(&drop);
        }
        written
    }
}

/// The drops folder: made if missing, a real folder, and on Unix
/// traverse-only to others, who reach a drop only by its random id.
fn root(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        bail!("{} is not a real folder", dir.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o711))
            .with_context(|| format!("setting {} traverse-only", dir.display()))?;
    }
    Ok(())
}

fn create_private_dir(dir: &Path) -> Result<()> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .with_context(|| format!("creating {}", dir.display()))
}

/// Read and traverse for `user` on the drop and what is in it.
fn grant(drop: &Path, user: &str) -> Result<()> {
    let output = std::process::Command::new("setfacl")
        .args(["-R", "-m", &format!("u:{user}:r-X"), "--"])
        .arg(drop)
        .output()
        .context("running setfacl")?;
    if !output.status.success() {
        bail!(
            "granting {user} read: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// A sortable, unguessable folder name: the time, then 128 random bits.
fn drop_id() -> String {
    let now = jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ");
    format!("{now}-{}", parvi_node_kit::identity::new_id())
}

/// The file's own name, kept to letters, digits, `.`, `_` and `-`, as CHROTE
/// keeps it; a name with nothing left is `file`.
fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut out = String::new();
    for c in base.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches(['.', '-', '_']);
    if out.is_empty() {
        "file".into()
    } else {
        out.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_keeps_only_its_own_safe_part() {
        assert_eq!(clean_name("screen shot (1).png"), "screen-shot-1-.png");
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name(r"C:\Users\me\notes.txt"), "notes.txt");
        assert_eq!(clean_name(".."), "file");
        assert_eq!(clean_name("Ä"), "file");
    }

    #[test]
    fn a_drop_is_private_and_a_stranger_is_refused() {
        let dir = std::env::temp_dir().join(format!("parvi-drops-{}", std::process::id()));
        let drops = Drops::new(dir.clone(), vec![]);
        let path = drops.write("a b.txt", "", b"hello").unwrap();
        assert_eq!(path.file_name().unwrap(), "a-b.txt");
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir), 0o711);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
        assert!(drops.write("x", "mallory", b"x").is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "nothing left");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
