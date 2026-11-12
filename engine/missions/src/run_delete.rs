//! Deleting a finished run: its record and what Parvi made for it.
use crate::Coordinator;
use std::io;
use std::path::PathBuf;

impl Coordinator {
    /// Delete a finished run: its event record, frozen mission, bindings,
    /// kept outputs and the folder Parvi made for it. A folder the operator
    /// gave stays, and a repository run is landed first so its branch is
    /// not lost.
    pub fn delete_run(&self, id: &str) -> Result<(), crate::Error> {
        (|| -> io::Result<()> {
            let run = self.run(id).map_err(crate::error::to_io)?;
            if !run.r#final {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the run is still going; abort it first",
                ));
            }
            if !run.repository.is_empty() && PathBuf::from(&run.cwd).exists() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the run still has its branch and worktree; land it first",
                ));
            }
            let root = &self.inner.root;
            let made = self
                .inner
                .settings
                .run_workspace_root
                .clone()
                .unwrap_or_else(|| root.join("workspaces"))
                .join(id);
            for dir in [root.join("artifacts").join(id), made] {
                match std::fs::remove_dir_all(&dir) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
            }
            // The record goes last: until then the run still lists and a
            // failed delete can be tried again.
            for end in ["bindings.toml", "snapshot.toml", "ndjson"] {
                match std::fs::remove_file(root.join("runs").join(format!("{id}.{end}"))) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
            }
            Ok(())
        })()
        .map_err(crate::error::io)
    }
}
