//! One JSON file per task in the node's own folder, written whole and at
//! once, as CHROTE's store keeps them (`tsk_*.json`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use parvi_scheduled_contract::Task;

/// Task files that do not read, by id, each with why.
pub type Unreadable = Vec<(String, anyhow::Error)>;

pub struct Store {
    dir: PathBuf,
}

/// `tsk_` and letters, digits, `_` and `-`: the only names a task file has.
pub fn safe_id(id: &str) -> bool {
    id.strip_prefix("tsk_").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

impl Store {
    pub fn new(dir: PathBuf) -> Store {
        Store { dir }
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        if !safe_id(id) {
            bail!("invalid task id");
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    /// Every task that reads, oldest first, then by id, and apart from
    /// them each file that does not, by id with its error. Only a folder
    /// that cannot be read fails the whole.
    pub fn list_all(&self) -> Result<(Vec<Task>, Unreadable)> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], vec![])),
            Err(e) => return Err(e).with_context(|| format!("reading {}", self.dir.display())),
        };
        let mut tasks = vec![];
        let mut broken = vec![];
        for entry in entries {
            let name = entry
                .with_context(|| format!("reading {}", self.dir.display()))?
                .file_name()
                .to_string_lossy()
                .into_owned();
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            if safe_id(id) {
                match self.get(id) {
                    Ok(task) => tasks.extend(task),
                    Err(e) => broken.push((id.to_string(), e)),
                }
            }
        }
        tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        broken.sort_by(|a, b| a.0.cmp(&b.0));
        Ok((tasks, broken))
    }

    /// The task, or `None` if there is no such file.
    pub fn get(&self, id: &str) -> Result<Option<Task>> {
        let path = self.path(id)?;
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let task = serde_json::from_slice(&raw)
            .with_context(|| format!("read scheduled task {}", path.display()))?;
        Ok(Some(task))
    }

    /// Writes the task whole: a reader sees the old file or the new one.
    pub fn save(&self, task: &Task) -> Result<()> {
        let path = self.path(&task.id)?;
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        let mut raw = serde_json::to_vec_pretty(task)?;
        raw.push(b'\n');
        let tmp = self
            .dir
            .join(format!(".tmp-{}-{}.json", task.id, std::process::id()));
        write_private(&tmp, &raw)?;
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        Ok(())
    }

    /// Removes the task; `false` if there was none.
    pub fn delete(&self, id: &str) -> Result<bool> {
        match std::fs::remove_file(self.path(id)?) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

/// A task holds prompts: readable by its owner only.
fn write_private(path: &Path, raw: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    file.write_all(raw)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_task_names_only() {
        assert!(safe_id("tsk_mbx1_ab12"));
        assert!(!safe_id("tsk_"));
        assert!(!safe_id("tsk_../x"));
        assert!(!safe_id("run_1"));
    }
}
