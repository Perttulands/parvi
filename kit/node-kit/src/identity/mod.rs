//! This node's identity: an id generated once and kept in the config dir,
//! the device it runs on, and its environment.

mod environment;
mod hostname;
mod ports;

pub use environment::Environment;
pub(crate) use hostname::hostname;
pub use ports::Ports;

use std::hash::{BuildHasher, RandomState};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

use parvi_core::NodeId;

/// Who this node is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Stable across restarts and upgrades: 32 hex digits.
    pub id: NodeId,
    /// The machine's name. Both nodes of one Windows machine name the same
    /// device, which the hostname does by default, since WSL takes Windows'.
    pub device: String,
    pub environment: Environment,
}

/// The file in the config dir that holds the node id.
const ID_FILE: &str = "node";

impl Identity {
    /// This node, with its id from `config_dir`, created there on first
    /// start, and `device` or else the hostname, or `localhost` without one.
    pub fn load(config_dir: &Path, device: Option<String>) -> Result<Identity> {
        Ok(Identity {
            id: NodeId(load_id(config_dir)?),
            device: device
                .or_else(hostname)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "localhost".to_owned()),
            environment: Environment::detect(),
        })
    }

    /// Where this node keeps its settings: `%APPDATA%\Parvi` on Windows,
    /// else `$XDG_CONFIG_HOME/parvi` or `~/.config/parvi`.
    pub fn config_dir() -> Result<PathBuf> {
        let var = |name| std::env::var_os(name).filter(|value| !value.is_empty());
        if cfg!(windows) {
            let appdata = var("APPDATA").context("APPDATA is not set")?;
            return Ok(PathBuf::from(appdata).join("Parvi"));
        }
        let base = match var("XDG_CONFIG_HOME") {
            Some(dir) => PathBuf::from(dir),
            None => PathBuf::from(var("HOME").context("HOME is not set")?).join(".config"),
        };
        Ok(base.join("parvi"))
    }
}

/// Reads the node id, or makes one. The id is written whole to a file of
/// its own and then linked into place, which only one of several nodes
/// starting at once with the same dir can do, so they all agree and none
/// reads a partial id. A file that holds no valid id is refused, not replaced:
/// replacing it would race with another node doing the same.
fn load_id(dir: &Path) -> Result<String> {
    let path = dir.join(ID_FILE);
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let id = match read_id(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            publish_id(&path)?;
            read_id(&path)
        }
        read => read,
    }
    .with_context(|| format!("reading {}", path.display()))?;
    if !is_id(&id) {
        anyhow::bail!(
            "{} holds no node id; delete it to make a new one",
            path.display()
        );
    }
    Ok(id)
}

fn read_id(path: &Path) -> std::io::Result<String> {
    Ok(std::fs::read_to_string(path)?.trim().to_owned())
}

fn is_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Writes a new id beside `path` and links it there, unless another node
/// got there first.
fn publish_id(path: &Path) -> Result<()> {
    let id = new_id();
    let staged = path.with_extension(format!("{id}.tmp"));
    let written = std::fs::File::create(&staged).and_then(|mut file| {
        file.write_all(format!("{id}\n").as_bytes())?;
        file.sync_all()
    });
    let linked = written
        .with_context(|| format!("writing {}", staged.display()))
        .and_then(|()| match std::fs::hard_link(&staged, path) {
            Err(error) if error.kind() != ErrorKind::AlreadyExists => {
                Err(error).with_context(|| format!("creating {}", path.display()))
            }
            _ => Ok(()),
        });
    let _ = std::fs::remove_file(&staged);
    linked
}

/// 128 random bits as hex, from the OS-seeded keys std hashes with.
pub fn new_id() -> String {
    let high = RandomState::new().hash_one(SystemTime::now());
    let low = RandomState::new().hash_one(std::process::id());
    format!("{high:016x}{low:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("parvi-node-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn keeps_the_node_id_it_made() {
        let dir = scratch("keeps");
        let first = Identity::load(&dir, None).unwrap();
        assert_eq!(first.id.as_str().len(), 32);
        assert!(first.id.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
        let again = Identity::load(&dir, None).unwrap();
        assert_eq!(again.id, first.id, "the id survives a restart");
        let elsewhere = scratch("elsewhere");
        assert_ne!(Identity::load(&elsewhere, None).unwrap().id, first.id);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }

    #[test]
    fn names_the_device_by_hostname_unless_told() {
        let dir = scratch("device");
        assert!(!Identity::load(&dir, None).unwrap().device.is_empty());
        let named = Identity::load(&dir, Some("rig".into())).unwrap();
        assert_eq!(named.device, "rig");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_an_id_file_without_an_id() {
        let dir = scratch("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        for text in ["", "0123abcd\n", "zz"] {
            std::fs::write(dir.join(ID_FILE), text).unwrap();
            let error = Identity::load(&dir, None).unwrap_err().to_string();
            assert!(error.contains("holds no node id"), "{text:?}: {error}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nodes_starting_at_once_agree_on_one_id() {
        for round in 0..20 {
            let dir = scratch(&format!("race-{round}"));
            let barrier = std::sync::Barrier::new(8);
            let ids: Vec<parvi_core::NodeId> = std::thread::scope(|scope| {
                let starts: Vec<_> = (0..8)
                    .map(|_| {
                        scope.spawn(|| {
                            barrier.wait();
                            Identity::load(&dir, None).unwrap().id
                        })
                    })
                    .collect();
                starts
                    .into_iter()
                    .map(|start| start.join().unwrap())
                    .collect()
            });
            assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");
            let files = std::fs::read_dir(&dir).unwrap().count();
            assert_eq!(files, 1, "only the id file is left");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
