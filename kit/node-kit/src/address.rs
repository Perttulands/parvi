//! Where this machine's node listens, for its own command line. A node
//! records the address it bound in the config dir; `parvi <noun> <verb>` and
//! `parvi mcp` read it, so a node on another loopback port than its
//! environment's needs no flag on every command. WSL with mirrored networking
//! needs that: Windows Tailscale holding a tailnet port also holds that port
//! for Linux, so `tailscale serve --tcp=7300` must forward to another one.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::identity::{Environment, Identity, Ports};

const FILE: &str = "listen";

/// The local node's address: the one it recorded, or else its environment's
/// default port on loopback.
pub fn local() -> String {
    Identity::config_dir()
        .ok()
        .and_then(|dir| recorded(&dir))
        .unwrap_or_else(|| format!("127.0.0.1:{}", Ports::default().of(Environment::detect())))
}

fn recorded(config_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config_dir.join(FILE)).ok()?;
    let addr: SocketAddr = text.trim().parse().ok()?;
    Some(addr.to_string())
}

/// Records `addr` as where this node listens until the returned guard
/// drops, which removes it again unless another node recorded since.
pub fn record(config_dir: &Path, addr: SocketAddr) -> Result<Recorded> {
    let path = config_dir.join(FILE);
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("creating {}", config_dir.display()))?;
    let next = config_dir.join(format!("{FILE}.{}", std::process::id()));
    std::fs::write(&next, format!("{addr}\n"))
        .with_context(|| format!("writing {}", next.display()))?;
    std::fs::rename(&next, &path).with_context(|| format!("writing {}", path.display()))?;
    Ok(Recorded { path, addr })
}

pub struct Recorded {
    path: PathBuf,
    addr: SocketAddr,
}

impl Drop for Recorded {
    fn drop(&mut self) {
        let ours = std::fs::read_to_string(&self.path)
            .is_ok_and(|text| text.trim().parse() == Ok(self.addr));
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_address_lasts_while_its_node_runs_and_not_over_a_later_one() {
        let dir = std::env::temp_dir().join(format!("parvi-address-{}", std::process::id()));
        let first: SocketAddr = "127.0.0.1:7310".parse().unwrap();
        let second: SocketAddr = "127.0.0.1:7311".parse().unwrap();

        let recorded_first = record(&dir, first).unwrap();
        assert_eq!(recorded(&dir).as_deref(), Some("127.0.0.1:7310"));

        let recorded_second = record(&dir, second).unwrap();
        drop(recorded_first);
        assert_eq!(recorded(&dir).as_deref(), Some("127.0.0.1:7311"));

        drop(recorded_second);
        assert_eq!(recorded(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
