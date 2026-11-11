//! Reads one Beads store through `bd --json`, run in its project folder.
//! Creates items through the same isolated store environment.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use super::tree;

/// How long one `bd` call may take before it counts as failed.
const TIMEOUT: Duration = Duration::from_secs(30);

/// The bd program: `bd` on the path, or another for tests.
#[derive(Debug, Clone)]
pub struct Bd {
    program: String,
}

impl Default for Bd {
    fn default() -> Bd {
        Bd::new("bd")
    }
}

impl Bd {
    pub fn new(program: impl Into<String>) -> Bd {
        Bd {
            program: program.into(),
        }
    }

    /// Every Bead in the store, open and closed, as `bd list` prints them.
    pub async fn list(&self, folder: &str) -> Result<String> {
        self.run(folder, &["list", "--status", "all", "--limit", "0"])
            .await
    }

    pub async fn create(
        &self,
        folder: &str,
        title: &str,
        description: &str,
        kind: &str,
    ) -> Result<String> {
        let output = self
            .run(
                folder,
                &["create", title, "--description", description, "-t", kind],
            )
            .await?;
        let value: serde_json::Value =
            serde_json::from_str(&output).context("bd's answer is not JSON")?;
        Ok(value["id"]
            .as_str()
            .context("bd did not return an id")?
            .to_owned())
    }

    /// The prefix the store gives its Beads' ids.
    pub async fn prefix(&self, folder: &str) -> Result<String> {
        let json = self.run(folder, &["config", "get", "issue_prefix"]).await?;
        let value: serde_json::Value =
            serde_json::from_str(&json).context("bd's answer is not JSON")?;
        match value["value"].as_str() {
            Some(prefix) if !prefix.is_empty() => Ok(prefix.to_owned()),
            _ => bail!("the Beads store in {folder} has no issue prefix"),
        }
    }

    /// Runs `bd --json <args>` in `folder`, which must hold a store of its
    /// own: bd would otherwise find a parent folder's. The environment's
    /// store overrides are dropped for the same reason.
    async fn run(&self, folder: &str, args: &[&str]) -> Result<String> {
        if !Path::new(folder).join(".beads").is_dir() {
            bail!("no Beads store in {folder}");
        }
        let mut command = Command::new(&self.program);
        command
            .arg("--json")
            .args(args)
            .current_dir(folder)
            .env_remove("BEADS_DIR")
            .env_remove("BEADS_DB")
            .env_remove("BD_DB")
            .stdin(std::process::Stdio::null());
        let output = tokio::time::timeout(TIMEOUT, tree::output(&mut command))
            .await
            .context("bd took too long")?
            .with_context(|| format!("running {}", self.program))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let trimmed = stdout.trim_start();
        if output.status.success() && (trimmed.starts_with('[') || trimmed.starts_with('{')) {
            return Ok(stdout.into_owned());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() && !stderr.trim().is_empty() {
            bail!(
                "{}",
                stderr
                    .trim()
                    .strip_prefix("Error: ")
                    .unwrap_or(stderr.trim())
            );
        }
        let said = serde_json::from_str::<serde_json::Value>(trimmed)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_owned));
        match said.or_else(|| {
            stderr
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_owned)
        }) {
            Some(line) => bail!("{}", line.strip_prefix("Error: ").unwrap_or(&line)),
            None => bail!("bd failed without saying why ({})", output.status),
        }
    }
}

/// What changes whenever the store does, so a watched store is read again
/// only after a write: the Dolt manifests of its databases, embedded
/// (`.beads/embeddeddolt/<db>`) or kept by a local Dolt server
/// (`.beads/dolt/<db>`). Any other layout, such as a store served from
/// elsewhere, has no marker Parvi can read cheaply: the error says why, and
/// the store is then read only when watching starts or on request.
pub fn marker(folder: &str) -> Result<Vec<u8>, String> {
    let beads = Path::new(folder).join(".beads");
    let mut manifests = Vec::new();
    for layout in ["embeddeddolt", "dolt"] {
        let Ok(databases) = std::fs::read_dir(beads.join(layout)) else {
            continue;
        };
        for database in databases.flatten() {
            let manifest = database.path().join(".dolt/noms/manifest");
            if manifest.is_file() {
                manifests.push(manifest);
            }
        }
    }
    if manifests.is_empty() {
        return Err(format!(
            "no Dolt manifest under {}, so Parvi cannot see when it changes",
            beads.display()
        ));
    }
    manifests.sort();
    let mut marker = Vec::new();
    for manifest in manifests {
        let bytes = std::fs::read(&manifest)
            .map_err(|error| format!("reading {}: {error}", manifest.display()))?;
        marker.extend_from_slice(manifest.as_os_str().as_encoded_bytes());
        marker.push(0);
        marker.extend_from_slice(&bytes);
        marker.push(0);
    }
    Ok(marker)
}
