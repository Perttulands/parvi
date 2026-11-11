//! Staging access and directory permissions replayed during publication.
use super::{error::FilesError, receive::Target};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
pub struct Attributes {
    pub target: Target,
    pub parts: Vec<String>,
    pub readonly: bool,
    pub mode: Option<u32>,
}

pub fn record(stage: &Path, path: &Path, attrs: &Attributes) -> Result<(), FilesError> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_file() {
        return apply(path, attrs);
    }
    if !meta.is_dir() {
        return Err(FilesError::Outside);
    }
    let directory = stage.join("directory-attributes");
    match fs::create_dir(&directory) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    if !fs::symlink_metadata(&directory)?.is_dir() {
        return Err(FilesError::Outside);
    }
    let parts =
        serde_json::to_vec(&attrs.parts).map_err(|e| FilesError::Io(std::io::Error::other(e)))?;
    let name = hex::encode(Sha256::digest(parts));
    let temp = directory.join(format!("{name}.tmp"));
    let data = serde_json::to_vec(attrs).map_err(|e| FilesError::Io(std::io::Error::other(e)))?;
    fs::write(&temp, data)?;
    // The same attributes can arrive again on a retry. The destination's
    // receiver serializes requests, so no two writes overlap.
    let record = directory.join(format!("{name}.json"));
    if record.exists() {
        fs::remove_file(&record)?;
    }
    fs::rename(temp, record)?;
    Ok(())
}

fn apply(path: &Path, attrs: &Attributes) -> Result<(), FilesError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(FilesError::Outside);
    }
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = attrs.mode {
            permissions.set_mode(mode & 0o777);
        } else {
            let mode = permissions.mode();
            permissions.set_mode(if attrs.readonly {
                mode & !0o222
            } else {
                mode | 0o200
            });
        }
    }
    #[cfg(not(unix))]
    permissions.set_readonly(attrs.readonly);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

/// Makes a staged tree accessible, top down, before replaying its chunks.
pub fn restore(path: &Path) -> Result<(), FilesError> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() && !meta.is_dir() {
        return Err(FilesError::Outside);
    }
    writable(path, &meta)?;
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            restore(&entry?.path())?;
        }
    }
    Ok(())
}

fn writable(path: &Path, meta: &fs::Metadata) -> Result<(), FilesError> {
    let mut permissions = meta.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let access = if meta.is_dir() { 0o700 } else { 0o600 };
        permissions.set_mode(permissions.mode() | access);
    }
    #[cfg(not(unix))]
    {
        // This branch clears the Windows readonly attribute; Unix mode bits
        // are handled above without making the file world writable.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
    }
    fs::set_permissions(path, permissions)?;
    Ok(())
}

/// A retry can interrupt permission application after publication. Restore
/// access top-down first, then apply recorded directory modes bottom-up.
pub fn finish(stage: &Path, destination: &Path, target: &Target) -> Result<(), FilesError> {
    let directory = stage.join("directory-attributes");
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.path().extension().is_none_or(|e| e != "json") {
            continue;
        }
        let attrs: Attributes = serde_json::from_slice(&fs::read(entry.path())?)
            .map_err(|e| FilesError::Io(std::io::Error::other(e)))?;
        if attrs.target != *target
            || attrs
                .parts
                .iter()
                .any(|name| !super::receive::valid_name(name))
        {
            return Err(FilesError::Outside);
        }
        records.push(attrs);
    }
    records.sort_by_key(|attrs| attrs.parts.len());
    for attrs in &records {
        let path = directory_path(destination, &attrs.parts)?;
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_dir() {
            return Err(FilesError::Outside);
        }
        writable(&path, &meta)?;
    }
    for attrs in records.iter().rev() {
        apply(&directory_path(destination, &attrs.parts)?, attrs)?;
    }
    Ok(())
}

fn directory_path(root: &Path, parts: &[String]) -> Result<PathBuf, FilesError> {
    let mut path = root.to_path_buf();
    for part in parts {
        if !fs::symlink_metadata(&path)?.is_dir() {
            return Err(FilesError::Outside);
        }
        path.push(part);
    }
    Ok(path)
}
