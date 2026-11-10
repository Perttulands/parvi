//! Sessions owned by a caller, identified across server restarts.
use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::control::PaneId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    pub name: String,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    /// Executable and arguments, without a shell wrapper.
    pub command: Vec<String>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generation {
    pub pid: u32,
    pub start_time: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub session_id: String,
    pub pane_id: PaneId,
    pub generation: Generation,
}

impl SessionSpec {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.name.is_empty() || self.name.contains([':', '.', '\0', '\n']) {
            bail!("invalid session name");
        }
        if self.command.is_empty()
            || self.command[0].is_empty()
            || self.command.iter().any(|s| s.contains('\0'))
        {
            bail!("command must name an executable and contain no NULs");
        }
        if self.cols == 0 || self.rows == 0 || self.cols > 10000 || self.rows > 10000 {
            bail!("invalid session size");
        }
        if self
            .env
            .iter()
            .any(|(k, v)| k.is_empty() || k.contains(['=', '\0']) || v.contains('\0'))
        {
            bail!("invalid environment variable");
        }
        Ok(())
    }
}

pub(crate) fn parse_ref(reply: &str) -> Result<SessionRef> {
    let mut words = reply.split_whitespace();
    let session_id = words.next().unwrap_or_default();
    let pane = words.next().unwrap_or_default();
    let pid = words.next().unwrap_or_default().parse()?;
    let start_time = words.next().unwrap_or_default().parse()?;
    if session_id
        .strip_prefix('$')
        .is_none_or(|n| n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()))
        || words.next().is_some()
    {
        bail!("invalid session identity from backend");
    }
    let pane_id = crate::control::parse_pane(pane)
        .ok_or_else(|| anyhow::anyhow!("invalid pane identity from backend"))?;
    Ok(SessionRef {
        session_id: session_id.to_owned(),
        pane_id,
        generation: Generation { pid, start_time },
    })
}
