//! A local copy stages chunks before publication. A retry verifies the staged
//! prefix before continuing.
use super::{
    Files,
    error::FilesError,
    receive::{self, Attributes, Probe, Target, WriteChunk},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::{io::AsyncReadExt, sync::watch};

pub use parvi_files_contract::{Progress, TransferRequest as Start};
/// Each transfer this node started, with what started it, so a failed one
/// can be resumed by its id alone.
pub type Jobs = Mutex<HashMap<String, Job>>;

pub struct Job {
    progress: watch::Sender<Progress>,
    start: Start,
}

pub fn start(files: Arc<Files>, request: Start) -> Result<(), FilesError> {
    if request.node != files.node {
        return Err(FilesError::OtherNode(request.node));
    }
    if request.target_node != files.node {
        return Err(FilesError::OtherNode(request.target_node));
    }
    if request.id.len() != 32 || !request.id.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(FilesError::Outside);
    }
    let mut jobs = files.jobs.lock().unwrap();
    if let Some(job) = jobs.get(&request.id)
        && job.progress.borrow().state == "running"
    {
        return Err(FilesError::Exists);
    }
    let initial = Progress {
        state: "running".into(),
        bytes: 0,
        total: 0,
        error: None,
    };
    let (tx, _) = watch::channel(initial);
    jobs.insert(
        request.id.clone(),
        Job {
            progress: tx.clone(),
            start: request.clone(),
        },
    );
    drop(jobs);
    tokio::spawn(async move {
        let node: String = request.target_node.chars().take(8).collect();
        let what = format!("{} to node {node}", request.path);
        let events = files.events.clone();
        let result = transfer(files, request, &tx).await;
        let mut status = tx.borrow().clone();
        match result {
            Ok(()) => status.state = "done".into(),
            Err(error) => {
                status.state = "failed".into();
                events.record(
                    "file_transfer",
                    true,
                    format!("Sending {what} failed: {error}"),
                );
                status.error = Some(error);
            }
        }
        tx.send_replace(status);
    });
    Ok(())
}

pub fn watch(files: &Files, id: &str) -> Option<watch::Receiver<Progress>> {
    files
        .jobs
        .lock()
        .unwrap()
        .get(id)
        .map(|job| job.progress.subscribe())
}

/// Starts a transfer this node started before again, as the same request:
/// the destination's staged prefix is verified and the copy goes on from it.
pub fn resume(files: Arc<Files>, id: &str) -> Result<(), FilesError> {
    let start = files
        .jobs
        .lock()
        .unwrap()
        .get(id)
        .map(|job| job.start.clone());
    let start = start.ok_or(FilesError::NotFound)?;
    self::start(files, start)
}

struct Item {
    path: PathBuf,
    parts: Vec<String>,
    folder: bool,
    size: u64,
    modified: Option<std::time::SystemTime>,
    readonly: bool,
    mode: Option<u32>,
}
fn scan(root: &Path) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    let mounts = super::find::mount_points();
    let mut pending = vec![(root.to_path_buf(), Vec::<String>::new())];
    while let Some((path, parts)) = pending.pop() {
        if path != root && mounts.contains(&path) {
            return Err(format!("{} is a mount point", path.display()));
        }
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !meta.is_file() && !meta.is_dir() {
            return Err(format!("{} is a link or special file", path.display()));
        }
        items.push(Item {
            path: path.clone(),
            parts: parts.clone(),
            folder: meta.is_dir(),
            size: meta.len(),
            modified: meta.modified().ok(),
            readonly: meta.permissions().readonly(),
            mode: file_mode(&meta),
        });
        if meta.is_dir() {
            let mut entries = fs::read_dir(&path)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            entries.sort_by_key(|e| e.file_name());
            for entry in entries.into_iter().rev() {
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "A name is not UTF-8".to_owned())?;
                if name.contains(['/', '\\']) {
                    return Err("A name cannot be transferred".into());
                }
                let mut child = parts.clone();
                child.push(name);
                pending.push((entry.path(), child));
            }
        }
    }
    Ok(items)
}

#[cfg(unix)]
fn file_mode(meta: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o777)
}
#[cfg(not(unix))]
fn file_mode(_: &fs::Metadata) -> Option<u32> {
    None
}

fn snapshot(items: &[Item]) -> Result<String, String> {
    let mut hash = Sha256::new();
    for item in items {
        hash.update(serde_json::to_vec(&item.parts).map_err(|e| e.to_string())?);
        hash.update([u8::from(item.folder)]);
        hash.update(item.size.to_le_bytes());
        hash.update([u8::from(item.readonly)]);
        hash.update(item.mode.unwrap_or_default().to_le_bytes());
        let modified = item
            .modified
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        hash.update(modified.to_le_bytes());
    }
    Ok(format!("{:x}", hash.finalize()))
}

async fn transfer(
    files: Arc<Files>,
    request: Start,
    tx: &watch::Sender<Progress>,
) -> Result<(), String> {
    let root = files
        .roots
        .existing(&request.path)
        .map_err(|e| e.to_string())?;
    // Reject a link selected as the top-level item too.
    if fs::symlink_metadata(&request.path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("Cannot transfer a link".into());
    }
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("A root cannot be transferred")?
        .to_owned();
    if request.target_node == files.node {
        let folder = files
            .roots
            .existing(&request.folder)
            .map_err(|e| e.to_string())?;
        if folder.starts_with(&root) {
            return Err("Cannot copy a folder into itself".into());
        }
    }
    let source = root.clone();
    let items = tokio::task::spawn_blocking(move || scan(&source))
        .await
        .map_err(|e| e.to_string())??;
    let total = items.iter().filter(|i| !i.folder).map(|i| i.size).sum();
    let source_snapshot = snapshot(&items)?;
    tx.send_modify(|p| p.total = total);
    let target = Target {
        folder: request.folder,
        id: request.id,
        name,
        source_node: files.node.clone(),
        source_path: request.path,
        snapshot: source_snapshot,
    };
    let complete = {
        let files = files.clone();
        let target = target.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = files.receiving.lock().unwrap();
            receive::prepare(&files.roots, &target).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??
    };
    if complete {
        tx.send_modify(|p| p.bytes = total);
        return Ok(());
    }
    let mut done = 0;
    for item in &items {
        if item.folder {
            send(
                &files,
                WriteChunk {
                    target: target.clone(),
                    parts: item.parts.clone(),
                    folder: true,
                    offset: 0,
                    size: 0,
                    data: vec![],
                },
            )
            .await?;
            continue;
        }
        let mut file = tokio::fs::File::open(&item.path)
            .await
            .map_err(|e| e.to_string())?;
        let probe = Probe {
            target: target.clone(),
            parts: item.parts.clone(),
        };
        let staged = {
            let files = files.clone();
            tokio::task::spawn_blocking(move || {
                let _guard = files.receiving.lock().unwrap();
                receive::offset(&files.roots, &probe).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??
        };
        if staged.done {
            return Ok(());
        }
        if staged.offset > item.size {
            return Err("The staged file is larger than the source".into());
        }
        let mut hash = Sha256::new();
        let mut left = staged.offset;
        while left > 0 {
            let mut data = vec![0; (left as usize).min(receive::CHUNK)];
            let count = file.read(&mut data).await.map_err(|e| e.to_string())?;
            if count == 0 {
                return Err("The source changed during transfer".into());
            }
            hash.update(&data[..count]);
            left -= count as u64;
        }
        if format!("{:x}", hash.finalize()) != staged.hash {
            return Err("The staged file differs from the source".into());
        }
        let mut offset = staged.offset;
        done += offset;
        tx.send_modify(|p| p.bytes = done);
        loop {
            let mut data = vec![0; receive::CHUNK];
            let count = file.read(&mut data).await.map_err(|e| e.to_string())?;
            data.truncate(count);
            if count == 0 && (offset != 0 || staged.offset != 0) {
                break;
            }
            send(
                &files,
                WriteChunk {
                    target: target.clone(),
                    parts: item.parts.clone(),
                    folder: false,
                    offset,
                    size: item.size,
                    data,
                },
            )
            .await?;
            offset += count as u64;
            done += count as u64;
            tx.send_modify(|p| p.bytes = done);
            if count == 0 {
                break;
            }
        }
        let now = file.metadata().await.map_err(|e| e.to_string())?;
        if offset != item.size || now.len() != item.size || now.modified().ok() != item.modified {
            return Err(format!("{} changed during transfer", item.path.display()));
        }
    }
    for item in items.iter().rev() {
        let attrs = Attributes {
            target: target.clone(),
            parts: item.parts.clone(),
            readonly: item.readonly,
            mode: item.mode,
        };
        let files = files.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = files.receiving.lock().unwrap();
            receive::attributes(&files.roots, &attrs).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;
    }
    let source = root;
    let final_items = tokio::task::spawn_blocking(move || scan(&source))
        .await
        .map_err(|e| e.to_string())??;
    if snapshot(&final_items)? != target.snapshot {
        return Err("The source changed during transfer".into());
    }
    tokio::task::spawn_blocking(move || {
        let _guard = files.receiving.lock().unwrap();
        receive::finish(&files.roots, &target).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(())
}

async fn send(files: &Arc<Files>, chunk: WriteChunk) -> Result<(), String> {
    let files = files.clone();
    tokio::task::spawn_blocking(move || {
        let _guard = files.receiving.lock().unwrap();
        receive::write(&files.roots, &chunk).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    #[tokio::test]
    async fn copies_a_tree_and_refuses_another_node() {
        let scratch = Scratch::new();
        let source = scratch.dir("source");
        fs::write(source.join("note.txt"), "hello").unwrap();
        let destination = scratch.dir("destination");
        let files = Arc::new(Files::new("local".into(), &[scratch.path()], None).unwrap());
        assert!(matches!(
            files.copy(
                source.to_str().unwrap(),
                "elsewhere",
                destination.to_str().unwrap()
            ),
            Err(FilesError::OtherNode(_))
        ));
        let id = files
            .copy(
                source.to_str().unwrap(),
                "local",
                destination.to_str().unwrap(),
            )
            .unwrap();
        let mut progress = watch(&files, &id).unwrap();
        while progress.borrow_and_update().state == "running" {
            progress.changed().await.unwrap();
        }
        let result = progress.borrow().clone();
        assert_eq!(result.state, "done", "{:?}", result.error);
        assert_eq!(result.bytes, 5);
        assert_eq!(
            fs::read(destination.join("source/note.txt")).unwrap(),
            b"hello"
        );
    }

    #[tokio::test]
    async fn copy_reports_a_conflict_without_sending_data() {
        let scratch = Scratch::new();
        let source = scratch.file("file.txt", "source contents");
        let destination = scratch.dir("destination");
        fs::write(destination.join("file.txt"), "original").unwrap();
        let files = Arc::new(Files::new("local".into(), &[scratch.path()], None).unwrap());
        let id = files
            .copy(
                source.to_str().unwrap(),
                "local",
                destination.to_str().unwrap(),
            )
            .unwrap();
        let mut progress = watch(&files, &id).unwrap();
        while progress.borrow_and_update().state == "running" {
            progress.changed().await.unwrap();
        }
        let result = progress.borrow().clone();
        assert_eq!(result.state, "failed");
        assert_eq!(result.bytes, 0);
        assert!(result.error.unwrap().contains("already has that name"));
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"original");
        assert!(
            !destination
                .join(format!(".parvi-transfer-{id}/data"))
                .exists()
        );
    }
}
