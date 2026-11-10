//! Independent, bounded snapshots of file and folder outputs.
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub const CAPTURE_BYTES: u64 = 32 * 1024 * 1024;

pub struct Budget {
    pub remaining: u64,
}
#[derive(Debug)]
pub struct NotKept(pub io::Error);
pub struct KeptCopy;

impl KeptCopy {
    /// Copy into a sibling staging directory before replacing an earlier copy.
    pub fn keep(
        path: &Path,
        kind: &str,
        parent: &Path,
        budget: &mut Budget,
    ) -> Result<PathBuf, NotKept> {
        Self::keep_cancelable(path, kind, parent, budget, &AtomicBool::new(false), &[])
    }
    pub(crate) fn keep_cancelable(
        path: &Path,
        kind: &str,
        parent: &Path,
        budget: &mut Budget,
        canceled: &AtomicBool,
        excluded: &[PathBuf],
    ) -> Result<PathBuf, NotKept> {
        snapshot(path, kind, parent, budget, canceled, excluded).map_err(NotKept)
    }
}

fn snapshot(
    path: &Path,
    kind: &str,
    parent: &Path,
    budget: &mut Budget,
    canceled: &AtomicBool,
    excluded: &[PathBuf],
) -> io::Result<PathBuf> {
    let source = fs::canonicalize(path)?;
    let meta = fs::metadata(&source)?;
    if (kind == "file" && !meta.is_file()) || (kind == "folder" && !meta.is_dir()) {
        return Err(io::Error::other("output kind mismatch"));
    }
    fs::create_dir_all(parent)?;
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
    let stage = parent.join(format!(".snapshot-{:x}", u64::from_ne_bytes(random)));
    fs::create_dir(&stage)?;
    let name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("root"));
    let staged = stage.join(name);
    let target = parent.join(name);
    let stage_real = fs::canonicalize(&stage)?;
    let target_real = fs::canonicalize(parent)?.join(name);
    let mut excluded: Vec<_> = excluded
        .iter()
        .map(|dir| fs::canonicalize(dir).unwrap_or_else(|_| dir.clone()))
        .filter(|dir| !source.starts_with(dir))
        .chain(std::iter::once(stage_real))
        .collect();
    if target_real != source && target_real.starts_with(&source) {
        excluded.push(target_real);
    }
    let original_budget = budget.remaining;
    let mut measured = Budget {
        remaining: original_budget,
    };
    let mut preflight = Copier {
        budget: &mut measured,
        canceled,
        excluded: excluded.clone(),
        frames: Vec::new(),
        measure: true,
        entries: 0,
    };
    let result = preflight.copy(&source, &staged).and_then(|()| {
        let mut copier = Copier {
            budget,
            canceled,
            excluded,
            frames: Vec::new(),
            measure: false,
            entries: 0,
        };
        copier.copy(&source, &staged)?;
        remove_tree(&target)?;
        fs::rename(&staged, &target)?;
        freeze_tree(&target)?;
        Ok(target)
    });
    let _ = remove_tree(&stage);
    if result.is_err() {
        budget.remaining = original_budget;
    }
    result
}

struct Copier<'a> {
    budget: &'a mut Budget,
    canceled: &'a AtomicBool,
    excluded: Vec<PathBuf>,
    frames: Vec<(PathBuf, PathBuf)>,
    measure: bool,
    entries: usize,
}
impl Copier<'_> {
    fn copy(&mut self, src: &Path, dst: &Path) -> io::Result<()> {
        if self.canceled.load(Ordering::Acquire) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "copy canceled"));
        }
        if self.excluded.iter().any(|p| src == p) {
            return Ok(());
        }
        self.entries += 1;
        if self.entries > 100_000 {
            return Err(io::Error::other("output capture has too many entries"));
        }
        let meta = fs::symlink_metadata(src)?;
        if meta.file_type().is_symlink() {
            let link = fs::read_link(src)?;
            let Ok(real) = fs::canonicalize(src) else {
                return if self.measure {
                    Ok(())
                } else {
                    symlink(&link, dst, false)
                };
            };
            for (from, to) in self.frames.iter().rev() {
                if let Ok(relative) = real.strip_prefix(from) {
                    let mapped = to.join(relative);
                    if self.measure {
                        return Ok(());
                    }
                    return symlink(
                        &relative_path(dst.parent().unwrap(), &mapped),
                        dst,
                        fs::metadata(&real).is_ok_and(|m| m.is_dir()),
                    );
                }
            }
            return self.copy(&real, dst);
        }
        if meta.is_dir() {
            if let Some((_, existing)) = self.frames.iter().find(|(from, _)| from == src) {
                return if self.measure {
                    Ok(())
                } else {
                    symlink(&relative_path(dst.parent().unwrap(), existing), dst, true)
                };
            }
            if !self.measure {
                fs::create_dir(dst)?;
            }
            self.frames.push((src.to_path_buf(), dst.to_path_buf()));
            for entry in fs::read_dir(src)? {
                let entry = entry?;
                self.copy(&entry.path(), &dst.join(entry.file_name()))?;
            }
            self.frames.pop();
        } else if meta.is_file() {
            if meta.len() > self.budget.remaining {
                return Err(io::Error::other("output capture exceeds 32 MiB budget"));
            }
            if self.measure {
                self.budget.remaining -= meta.len();
                return Ok(());
            }
            let mut options = fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NONBLOCK);
            }
            let mut input = options.open(src)?;
            let opened = input.metadata()?;
            if !opened.is_file() {
                return Err(io::Error::other("output changed while copying"));
            }
            if opened.len() > self.budget.remaining {
                return Err(io::Error::other("output capture exceeds 32 MiB budget"));
            }
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dst)?;
            if reflink(&input, &output) {
                if self.canceled.load(Ordering::Acquire) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "copy canceled"));
                }
                let len = output.metadata()?.len();
                if len > self.budget.remaining {
                    return Err(io::Error::other("output capture exceeds 32 MiB budget"));
                }
                self.budget.remaining -= len;
                fs::set_permissions(dst, opened.permissions())?;
                return Ok(());
            }
            let mut buffer = [0u8; 65536];
            loop {
                if self.canceled.load(Ordering::Acquire) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "copy canceled"));
                }
                let n = input.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                if n as u64 > self.budget.remaining {
                    return Err(io::Error::other("output capture exceeds 32 MiB budget"));
                }
                output.write_all(&buffer[..n])?;
                self.budget.remaining -= n as u64;
            }
            fs::set_permissions(dst, opened.permissions())?;
        }
        Ok(())
    }
}
#[cfg(target_os = "linux")]
fn reflink(input: &fs::File, output: &fs::File) -> bool {
    use std::os::fd::AsRawFd;
    // FICLONE creates a copy-on-write file, never a shared mutable inode.
    unsafe { libc::ioctl(output.as_raw_fd(), 0x4004_9409, input.as_raw_fd()) == 0 }
}
#[cfg(not(target_os = "linux"))]
fn reflink(_: &fs::File, _: &fs::File) -> bool {
    false
}
pub(crate) fn freeze_tree(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            freeze_tree(&entry?.path())?;
        }
    }
    readonly(path)
}
pub(crate) fn readonly(path: &Path) -> io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() & !0o222);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
}
pub(crate) fn remove_tree(path: &Path) -> io::Result<()> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if meta.is_dir() {
        let mut perm = meta.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perm.set_mode(perm.mode() | 0o700);
        }
        #[cfg(not(unix))]
        {
            // This branch clears the Windows readonly attribute; the lint's
            // world-writable risk is Unix's, handled above.
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
        }
        fs::set_permissions(path, perm)?;
        for entry in fs::read_dir(path)? {
            remove_tree(&entry?.path())?;
        }
        fs::remove_dir(path)
    } else {
        remove_file(path)
    }
}
#[cfg(not(windows))]
fn remove_file(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}
#[cfg(windows)]
fn remove_file(path: &Path) -> io::Result<()> {
    use std::os::{windows::fs::OpenOptionsExt, windows::io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FileDispositionInfoEx,
        SetFileInformationByHandle,
    };
    if fs::remove_file(path).is_ok() {
        return Ok(());
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .access_mode(DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // Delete this directory entry without changing an attribute shared by hard links.
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileDispositionInfoEx,
            (&raw const disposition).cast(),
            std::mem::size_of_val(&disposition) as u32,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let mut a = from.components().peekable();
    let mut b = to.components().peekable();
    while a.peek().is_some() && a.peek() == b.peek() {
        a.next();
        b.next();
    }
    let mut result = PathBuf::new();
    for _ in a {
        result.push("..");
    }
    for c in b {
        result.push(c.as_os_str());
    }
    result
}
#[cfg(unix)]
fn symlink(src: &Path, dst: &Path, _: bool) -> io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}
#[cfg(windows)]
fn symlink(src: &Path, dst: &Path, dir: bool) -> io::Result<()> {
    if dir {
        std::os::windows::fs::symlink_dir(src, dst)
    } else {
        std::os::windows::fs::symlink_file(src, dst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scratch() -> PathBuf {
        let mut id = [0u8; 8];
        getrandom::fill(&mut id).unwrap();
        let root =
            std::env::temp_dir().join(format!("parvi-102-copy-{:x}", u64::from_ne_bytes(id)));
        fs::create_dir(&root).unwrap();
        root
    }
    #[test]
    fn copy_is_independent_and_budget_is_enforced() {
        let root = scratch();
        let source = root.join("file");
        fs::write(&source, b"original").unwrap();
        let mut budget = Budget { remaining: 8 };
        let copy = KeptCopy::keep(&source, "file", &root.join("kept"), &mut budget).unwrap();
        assert_eq!(budget.remaining, 0);
        fs::write(&source, b"changed!").unwrap();
        assert_eq!(fs::read(&copy).unwrap(), b"original");
        assert!(fs::metadata(&copy).unwrap().permissions().readonly());
        assert!(KeptCopy::keep(&source, "file", &root.join("other"), &mut budget).is_err());
        remove_tree(&root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn links_inside_stay_links_and_external_targets_are_copied() {
        use std::os::unix::fs::symlink;
        let root = scratch();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("inside"), b"inside").unwrap();
        fs::write(root.join("outside"), b"outside").unwrap();
        symlink(source.join("inside"), source.join("internal")).unwrap();
        symlink(root.join("outside"), source.join("external")).unwrap();
        symlink("missing", source.join("dangling")).unwrap();
        let mut budget = Budget { remaining: 100 };
        let copy = KeptCopy::keep(&source, "folder", &root.join("kept"), &mut budget).unwrap();
        assert!(
            fs::symlink_metadata(copy.join("internal"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(copy.join("internal")).unwrap(), b"inside");
        assert!(
            fs::symlink_metadata(copy.join("external"))
                .unwrap()
                .is_file()
        );
        assert_eq!(fs::read(copy.join("external")).unwrap(), b"outside");
        assert_eq!(
            fs::read_link(copy.join("dangling")).unwrap(),
            Path::new("missing")
        );
        remove_tree(&root).unwrap();
    }
    #[test]
    fn source_containing_the_artifact_tree_does_not_copy_itself() {
        let root = scratch();
        let source = root.join("project");
        let excluded = source.join("artifacts/run");
        fs::create_dir_all(&excluded).unwrap();
        fs::write(source.join("work"), "work").unwrap();
        let mut budget = Budget { remaining: 1024 };
        let copy = KeptCopy::keep_cancelable(
            &source,
            "folder",
            &excluded.join("outputs/port"),
            &mut budget,
            &AtomicBool::new(false),
            std::slice::from_ref(&excluded),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(copy.join("work")).unwrap(), "work");
        assert!(!copy.join("artifacts/run").exists());
        remove_tree(&root).unwrap();
    }
    #[test]
    fn replacing_an_output_used_as_its_own_source_copies_before_removing_it() {
        let root = scratch();
        let parent = root.join("port");
        fs::create_dir(&parent).unwrap();
        let source = parent.join("result");
        fs::write(&source, "original").unwrap();
        let mut budget = Budget { remaining: 1024 };
        let copy = KeptCopy::keep(&source, "file", &parent, &mut budget).unwrap();
        assert_eq!(copy, source);
        assert_eq!(fs::read_to_string(copy).unwrap(), "original");
        remove_tree(&root).unwrap();
    }
    #[test]
    fn canceled_copy_leaves_no_snapshot_or_spent_budget() {
        let root = scratch();
        let source = root.join("source");
        fs::write(&source, "work").unwrap();
        let canceled = AtomicBool::new(true);
        let mut budget = Budget { remaining: 1024 };
        assert!(
            KeptCopy::keep_cancelable(
                &source,
                "file",
                &root.join("kept"),
                &mut budget,
                &canceled,
                &[]
            )
            .is_err()
        );
        assert_eq!(budget.remaining, 1024);
        assert!(!root.join("kept/source").exists());
        remove_tree(&root).unwrap();
    }
    #[test]
    fn attempt_directory_does_not_include_capture_staging() {
        let root = scratch();
        let source = root.join("attempt");
        let stage = source.join(".capture-test");
        fs::create_dir_all(&stage).unwrap();
        fs::write(source.join("note"), "work").unwrap();
        let mut budget = Budget { remaining: 1024 };
        let copy = KeptCopy::keep_cancelable(
            &source,
            "folder",
            &stage.join("port"),
            &mut budget,
            &AtomicBool::new(false),
            std::slice::from_ref(&stage),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(copy.join("note")).unwrap(), "work");
        assert!(!copy.join(".capture-test").exists());
        remove_tree(&root).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn replacing_a_readonly_hardlinked_source_preserves_the_other_link() {
        let root = scratch();
        let parent = root.join("port");
        fs::create_dir(&parent).unwrap();
        let source = parent.join("result");
        let shared = root.join("shared");
        fs::write(&source, "original").unwrap();
        fs::hard_link(&source, &shared).unwrap();
        let mut perm = fs::metadata(&source).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&source, perm).unwrap();
        let mut budget = Budget { remaining: 1024 };
        let copy = KeptCopy::keep(&source, "file", &parent, &mut budget).unwrap();
        assert!(fs::metadata(&shared).unwrap().permissions().readonly());
        assert_eq!(fs::read_to_string(&shared).unwrap(), "original");
        assert_eq!(fs::read_to_string(copy).unwrap(), "original");
        remove_tree(&root).unwrap();
    }
}
