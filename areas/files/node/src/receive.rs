//! Destination staging for the bridge. Replayed chunks must match what was
//! already written, so a retry never silently combines different versions.
use super::{error::FilesError, roots::Roots};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub const CHUNK: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Target {
    pub folder: String,
    pub id: String,
    pub name: String,
    pub source_node: String,
    pub source_path: String,
    pub snapshot: String,
}
#[derive(Deserialize, Serialize)]
pub struct WriteChunk {
    pub target: Target,
    pub parts: Vec<String>,
    pub folder: bool,
    pub offset: u64,
    pub size: u64,
    pub data: Vec<u8>,
}

#[derive(Deserialize, Serialize)]
pub struct Probe {
    pub target: Target,
    pub parts: Vec<String>,
}
#[derive(Deserialize, Serialize)]
pub struct Offset {
    pub offset: u64,
    pub hash: String,
    pub done: bool,
}

pub fn offset(roots: &Roots, probe: &Probe) -> Result<Offset, FilesError> {
    let (_, stage) = stage(roots, &probe.target)?;
    if stage.join("done").exists() {
        return Ok(Offset {
            offset: 0,
            hash: String::new(),
            done: true,
        });
    }
    let path = child(&stage, &probe.parts)?;
    let mut hash = Sha256::new();
    let mut length = 0;
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            if !meta.file_type().is_file() {
                return Err(FilesError::NotFile);
            }
            let mut file = fs::File::open(path)?;
            let mut buf = [0; 65536];
            loop {
                let count = file.read(&mut buf)?;
                if count == 0 {
                    break;
                }
                hash.update(&buf[..count]);
                length += count as u64;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(Offset {
        offset: length,
        hash: format!("{:x}", hash.finalize()),
        done: false,
    })
}

pub use super::access::Attributes;

pub fn attributes(roots: &Roots, attrs: &Attributes) -> Result<(), FilesError> {
    let (_, stage) = stage(roots, &attrs.target)?;
    if stage.join("done").exists() {
        return Ok(());
    }
    super::access::record(&stage, &child(&stage, &attrs.parts)?, attrs)
}

pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0'])
}
fn stage(roots: &Roots, target: &Target) -> Result<(PathBuf, PathBuf), FilesError> {
    if target.id.len() != 32
        || !target.id.bytes().all(|c| c.is_ascii_hexdigit())
        || !valid_name(&target.name)
    {
        return Err(FilesError::Outside);
    }
    let folder = roots.existing(&target.folder)?;
    if !folder.is_dir() {
        return Err(FilesError::NotFolder);
    }
    let stage = folder.join(format!(".parvi-transfer-{}", target.id));
    match fs::create_dir(&stage) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    if !fs::symlink_metadata(&stage)?.file_type().is_dir() {
        return Err(FilesError::Outside);
    }
    let manifest = stage.join("manifest.json");
    let data = serde_json::to_vec(target).map_err(|e| FilesError::Io(std::io::Error::other(e)))?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&manifest)
    {
        Ok(mut file) => file.write_all(&data)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(&manifest)?.file_type().is_file()
                || fs::read(&manifest)? != data
            {
                return Err(FilesError::Exists);
            }
        }
        Err(e) => return Err(e.into()),
    }
    Ok((folder, stage))
}

fn child(stage: &Path, parts: &[String]) -> Result<PathBuf, FilesError> {
    let mut path = stage.join("data");
    for part in parts {
        if !valid_name(part) {
            return Err(FilesError::Outside);
        }
        let meta = fs::symlink_metadata(&path)?;
        if !meta.file_type().is_dir() {
            return Err(FilesError::Outside);
        }
        path.push(part);
    }
    Ok(path)
}

pub fn write(roots: &Roots, chunk: &WriteChunk) -> Result<(), FilesError> {
    if chunk.data.len() > CHUNK {
        return Err(FilesError::TooLarge);
    }
    if chunk
        .offset
        .checked_add(chunk.data.len() as u64)
        .is_none_or(|end| end > chunk.size)
    {
        return Err(FilesError::TooLarge);
    }
    let (_, stage) = stage(roots, &chunk.target)?;
    if stage.join("done").exists() {
        return Ok(());
    }
    let path = child(&stage, &chunk.parts)?;
    if chunk.folder {
        if !chunk.data.is_empty() || chunk.offset != 0 {
            return Err(FilesError::NotFolder);
        }
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        if !fs::symlink_metadata(path)?.file_type().is_dir() {
            return Err(FilesError::Outside);
        }
        return Ok(());
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    if !fs::symlink_metadata(&path)?.file_type().is_file() {
        return Err(FilesError::Outside);
    }
    let length = file.metadata()?.len();
    if length > chunk.size {
        return Err(FilesError::Exists);
    }
    if chunk.offset > length {
        return Err(FilesError::NotFound);
    }
    let overlap = (length - chunk.offset).min(chunk.data.len() as u64) as usize;
    file.seek(SeekFrom::Start(chunk.offset))?;
    let mut old = vec![0; overlap];
    file.read_exact(&mut old)?;
    if old != chunk.data[..overlap] {
        return Err(FilesError::Exists);
    }
    file.write_all(&chunk.data[overlap..])?;
    Ok(())
}

/// Prepare a retry: recognize a published item or make staged items writable
/// again before replay. The source reapplies the requested attributes later.
pub fn prepare(roots: &Roots, target: &Target) -> Result<bool, FilesError> {
    let (folder, stage) = stage(roots, target)?;
    if published(&folder, &stage, target)? {
        return Ok(true);
    }
    // Refuse a known clash before the source sends any file data. Keep the
    // final no-replace rename too: another process can claim the name later.
    match fs::symlink_metadata(folder.join(&target.name)) {
        Ok(_) => return Err(FilesError::Exists),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    match fs::symlink_metadata(stage.join("data")) {
        Ok(_) => super::access::restore(&stage.join("data"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(false)
}

/// A publishing record is written before the rename. When data is gone, the
/// destination must still have its recorded identity before completion is
/// acknowledged. In particular, an independently created destination cannot
/// turn an interrupted or failed rename into a successful transfer.
fn published(folder: &Path, stage: &Path, target: &Target) -> Result<bool, FilesError> {
    if stage.join("done").exists() {
        return Ok(true);
    }
    let record = stage.join("publishing.json");
    let bytes = match fs::read(record) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let expected: super::identity::Identity =
        serde_json::from_slice(&bytes).map_err(|e| FilesError::Io(std::io::Error::other(e)))?;
    let data = stage.join("data");
    match fs::symlink_metadata(&data) {
        Ok(_) => {
            if super::identity::identity(&data)? != expected {
                return Err(FilesError::Exists);
            }
            Ok(false)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if super::identity::identity(&folder.join(&target.name))? != expected {
                return Err(FilesError::Exists);
            }
            super::access::finish(stage, &folder.join(&target.name), target)?;
            fs::write(stage.join("done"), b"done")?;
            Ok(true)
        }
        Err(e) => Err(e.into()),
    }
}

pub fn finish(roots: &Roots, target: &Target) -> Result<(), FilesError> {
    let (folder, stage) = stage(roots, target)?;
    if published(&folder, &stage, target)? {
        return Ok(());
    }
    let destination = folder.join(&target.name);
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(FilesError::Exists);
    }
    let data = stage.join("data");
    record_publication(&stage)?;
    super::disk::rename_new(&data, &destination)?;
    if published(&folder, &stage, target)? {
        Ok(())
    } else {
        Err(FilesError::NotFound)
    }
}

fn record_publication(stage: &Path) -> Result<(), FilesError> {
    let record = stage.join("publishing.json");
    if !record.exists() {
        let identity = super::identity::identity(&stage.join("data"))?;
        let temporary = stage.join("publishing.tmp");
        let bytes =
            serde_json::to_vec(&identity).map_err(|e| FilesError::Io(std::io::Error::other(e)))?;
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary, record)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    fn target(folder: &Path) -> Target {
        Target {
            folder: folder.to_str().unwrap().into(),
            id: "a".repeat(32),
            name: "copy.txt".into(),
            source_node: "source".into(),
            source_path: "/source.txt".into(),
            snapshot: "one".into(),
        }
    }
    fn chunk(target: &Target, offset: u64, data: &[u8]) -> WriteChunk {
        WriteChunk {
            target: target.clone(),
            parts: vec![],
            folder: false,
            offset,
            size: 6,
            data: data.into(),
        }
    }
    #[test]
    fn replay_resumes_and_never_exposes_a_partial_file() {
        let scratch = Scratch::new();
        let dir = scratch.dir("destination");
        let roots = Roots::new(std::slice::from_ref(&dir), None).unwrap();
        let target = target(&dir);
        write(&roots, &chunk(&target, 0, b"abc")).unwrap();
        let staged = offset(
            &roots,
            &Probe {
                target: target.clone(),
                parts: vec![],
            },
        )
        .unwrap();
        assert_eq!(staged.offset, 3);
        assert_eq!(staged.hash, format!("{:x}", Sha256::digest(b"abc")));
        assert!(!dir.join("copy.txt").exists());
        write(&roots, &chunk(&target, 0, b"abcd")).unwrap();
        assert!(matches!(
            write(&roots, &chunk(&target, 0, b"bad")),
            Err(FilesError::Exists)
        ));
        assert!(matches!(
            write(&roots, &chunk(&target, 5, b"f")),
            Err(FilesError::NotFound)
        ));
        write(&roots, &chunk(&target, 4, b"ef")).unwrap();
        finish(&roots, &target).unwrap();
        assert_eq!(fs::read(dir.join("copy.txt")).unwrap(), b"abcdef");
        write(&roots, &chunk(&target, 0, b"abc")).unwrap();
        finish(&roots, &target).unwrap();
    }
    #[test]
    fn refuses_collision_changed_snapshot_and_escaping_paths() {
        let scratch = Scratch::new();
        let dir = scratch.dir("destination");
        let roots = Roots::new(std::slice::from_ref(&dir), None).unwrap();
        let target = target(&dir);
        write(&roots, &chunk(&target, 0, b"abc")).unwrap();
        let mut changed = target.clone();
        changed.snapshot = "two".into();
        assert!(matches!(
            write(&roots, &chunk(&changed, 0, b"abc")),
            Err(FilesError::Exists)
        ));
        let mut escape = chunk(&target, 0, b"abc");
        escape.parts = vec!["..".into()];
        assert!(matches!(write(&roots, &escape), Err(FilesError::Outside)));
        fs::write(dir.join("copy.txt"), "keep").unwrap();
        assert!(matches!(finish(&roots, &target), Err(FilesError::Exists)));
        assert_eq!(fs::read(dir.join("copy.txt")).unwrap(), b"keep");
    }
    #[test]
    fn prepare_rejects_existing_destination_before_writing_data() {
        let scratch = Scratch::new();
        let dir = scratch.dir("destination");
        let roots = Roots::new(std::slice::from_ref(&dir), None).unwrap();
        let target = target(&dir);
        fs::write(dir.join("copy.txt"), b"original").unwrap();
        assert!(matches!(prepare(&roots, &target), Err(FilesError::Exists)));
        assert_eq!(fs::read(dir.join("copy.txt")).unwrap(), b"original");
        assert!(
            !dir.join(format!(".parvi-transfer-{}/data", target.id))
                .exists()
        );
        fs::remove_file(dir.join("copy.txt")).unwrap();
        assert!(!prepare(&roots, &target).unwrap());
        write(&roots, &chunk(&target, 0, b"abcdef")).unwrap();
        finish(&roots, &target).unwrap();
        // A retry after publication must still recognize its own destination.
        assert!(prepare(&roots, &target).unwrap());
    }
    #[test]
    fn resume_recognises_rename_before_completion_marker_without_overwriting() {
        let scratch = Scratch::new();
        let dir = scratch.dir("destination");
        let roots = Roots::new(std::slice::from_ref(&dir), None).unwrap();
        let target = target(&dir);
        write(&roots, &chunk(&target, 0, b"abcdef")).unwrap();
        let (_, staged) = stage(&roots, &target).unwrap();
        record_publication(&staged).unwrap();
        // Another process claims the name after intent was recorded.
        fs::write(dir.join("copy.txt"), "other").unwrap();
        assert!(matches!(prepare(&roots, &target), Err(FilesError::Exists)));
        assert!(matches!(finish(&roots, &target), Err(FilesError::Exists)));
        assert_eq!(fs::read(dir.join("copy.txt")).unwrap(), b"other");
        fs::remove_file(dir.join("copy.txt")).unwrap();
        // The node stops immediately after rename and before writing `done`.
        super::super::disk::rename_new(&staged.join("data"), &dir.join("copy.txt")).unwrap();
        let restarted = Roots::new(std::slice::from_ref(&dir), None).unwrap();
        assert!(prepare(&restarted, &target).unwrap());
        assert!(staged.join("done").exists());
        assert_eq!(fs::read(dir.join("copy.txt")).unwrap(), b"abcdef");
        finish(&restarted, &target).unwrap();
    }

    #[test]
    fn resume_refuses_a_different_file_after_publication() {
        let scratch = Scratch::new();
        let dir = scratch.dir("destination");
        let roots = Roots::new(std::slice::from_ref(&dir), None).unwrap();
        let target = target(&dir);
        write(&roots, &chunk(&target, 0, b"abcdef")).unwrap();
        let (_, staged) = stage(&roots, &target).unwrap();
        record_publication(&staged).unwrap();
        super::super::disk::rename_new(&staged.join("data"), &dir.join("copy.txt")).unwrap();
        fs::rename(dir.join("copy.txt"), dir.join("original")).unwrap();
        fs::write(dir.join("copy.txt"), "other").unwrap();
        assert!(matches!(prepare(&roots, &target), Err(FilesError::Exists)));
        assert_eq!(fs::read(dir.join("copy.txt")).unwrap(), b"other");
    }

    #[cfg(unix)]
    #[test]
    fn resume_restores_staging_access_after_restrictive_attributes() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new();
        let dir = scratch.dir("destination");
        let roots = Roots::new(std::slice::from_ref(&dir), None).unwrap();
        let target = target(&dir);
        let folder = WriteChunk {
            target: target.clone(),
            parts: vec![],
            folder: true,
            offset: 0,
            size: 0,
            data: vec![],
        };
        write(&roots, &folder).unwrap();
        let mut file = chunk(&target, 0, b"abcdef");
        file.parts = vec!["file".into()];
        write(&roots, &file).unwrap();
        attributes(
            &roots,
            &Attributes {
                target: target.clone(),
                parts: file.parts.clone(),
                readonly: true,
                mode: Some(0o444),
            },
        )
        .unwrap();
        attributes(
            &roots,
            &Attributes {
                target: target.clone(),
                parts: vec![],
                readonly: true,
                mode: Some(0o555),
            },
        )
        .unwrap();
        let (_, staged) = stage(&roots, &target).unwrap();
        fs::set_permissions(staged.join("data"), fs::Permissions::from_mode(0o555)).unwrap();
        let restarted = Roots::new(std::slice::from_ref(&dir), None).unwrap();
        assert!(!prepare(&restarted, &target).unwrap());
        let (_, staged) = stage(&roots, &target).unwrap();
        assert_eq!(
            fs::metadata(staged.join("data"))
                .unwrap()
                .permissions()
                .mode()
                & 0o700,
            0o700
        );
        assert_eq!(
            fs::metadata(staged.join("data/file"))
                .unwrap()
                .permissions()
                .mode()
                & 0o600,
            0o600
        );
        file.parts = vec!["new".into()];
        write(&restarted, &file).unwrap();
        attributes(
            &roots,
            &Attributes {
                target: target.clone(),
                parts: vec!["file".into()],
                readonly: true,
                mode: Some(0o444),
            },
        )
        .unwrap();
        attributes(
            &roots,
            &Attributes {
                target: target.clone(),
                parts: vec![],
                readonly: true,
                mode: Some(0o555),
            },
        )
        .unwrap();
        // Publication was interrupted after rename while permissions were being applied.
        record_publication(&staged).unwrap();
        super::super::disk::rename_new(&staged.join("data"), &dir.join("copy.txt")).unwrap();
        fs::set_permissions(dir.join("copy.txt"), fs::Permissions::from_mode(0o000)).unwrap();
        assert!(prepare(&restarted, &target).unwrap());
        assert_eq!(
            fs::metadata(dir.join("copy.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o555
        );
        assert_eq!(
            fs::metadata(dir.join("copy.txt/file"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o444
        );
        // Scratch cleanup requires writable permissions.
        fs::set_permissions(dir.join("copy.txt"), fs::Permissions::from_mode(0o755)).unwrap();
    }
}
