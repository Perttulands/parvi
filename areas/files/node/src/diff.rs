//! A file's bounded unified diff against its repository's committed HEAD.
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use super::{error::FilesError, roots::Roots};

const LIMIT: u64 = 1 << 20;

pub use parvi_files_contract::Diff;

pub fn read(roots: &Roots, requested: &str) -> Result<Diff, FilesError> {
    let path = roots.existing(requested)?;
    if roots
        .list()
        .iter()
        .any(|root| Path::new(&root.path) == path)
    {
        return Err(FilesError::Root);
    }
    if !path.is_file() {
        return Err(FilesError::NotFile);
    }
    let mut result = Diff {
        path: path.to_string_lossy().into_owned(),
        repository: String::new(),
        diff: String::new(),
        truncated: false,
        error: None,
    };
    let Some(repository) = path
        .parent()
        .and_then(|parent| parent.ancestors().find(|p| p.join(".git").exists()))
    else {
        return Ok(result);
    };
    result.repository = repository.to_string_lossy().into_owned();
    let relative = path
        .strip_prefix(repository)
        .expect("repository is an ancestor");
    let mut child = match Command::new("git")
        .current_dir(repository)
        .args([
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
            "--",
        ])
        .arg(relative)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            result.error = Some(e.to_string());
            return Ok(result);
        }
    };
    let stderr = child.stderr.take().expect("piped stderr");
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.take(LIMIT + 1).read_to_end(&mut bytes);
        bytes
    });
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("piped stdout")
        .take(LIMIT + 1)
        .read_to_end(&mut bytes);
    result.truncated = bytes.len() as u64 > LIMIT;
    if result.truncated || read.is_err() {
        let _ = child.kill();
    }
    let status = child.wait();
    let errors = errors.join().unwrap_or_default();
    bytes.truncate(LIMIT as usize);
    result.diff = String::from_utf8_lossy(&bytes).into_owned();
    if let Err(e) = read {
        result.error = Some(e.to_string());
    } else if !result.truncated {
        match status {
            Ok(status) if !status.success() => {
                let message = String::from_utf8_lossy(&errors).trim().to_owned();
                result.error = Some(if message.is_empty() {
                    format!("git exited with {status}")
                } else {
                    message
                });
            }
            Err(e) => result.error = Some(e.to_string()),
            _ => {}
        }
    }
    Ok(result)
}
