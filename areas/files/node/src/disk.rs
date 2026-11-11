//! What Files does to the disk, each under the rules of [`Roots`]. Blocking:
//! callers run these off the async threads.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::UNIX_EPOCH;

use super::error::FilesError;
use super::roots::Roots;

/// One thing in a folder. A link shows as what it points to; a link out of
/// the roots, or to nothing, is left out.
pub use parvi_files_contract::Entry;

pub use parvi_files_contract::Listing;

/// A folder's entries, by name. Names that are not UTF-8 are left out: the
/// browser could not name them back.
pub fn list(roots: &Roots, path: &str) -> Result<Listing, FilesError> {
    let folder = roots.existing(path)?;
    if !folder.is_dir() {
        return Err(FilesError::NotFolder);
    }
    let mut entries = Vec::new();
    for item in fs::read_dir(&folder)? {
        let item = item?;
        let Ok(name) = item.file_name().into_string() else {
            continue;
        };
        let Some(meta) = shown(roots, &item.path()) else {
            continue;
        };
        entries.push(Entry {
            name,
            folder: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified: modified(&meta),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Listing {
        path: text(&folder)?,
        entries,
    })
}

/// What a listing shows for an entry: its own metadata, or for a link that
/// of its target while the target is under a root.
fn shown(roots: &Roots, path: &Path) -> Option<fs::Metadata> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_symlink() {
        return Some(meta);
    }
    let target = path.canonicalize().ok()?;
    roots
        .contains(&target)
        .then(|| fs::metadata(&target).ok())?
}

/// A file's bytes, refused above `limit`.
pub fn read(roots: &Roots, path: &str, limit: u64) -> Result<Vec<u8>, FilesError> {
    let file = roots.existing(path)?;
    let meta = fs::metadata(&file)?;
    if !meta.is_file() {
        return Err(FilesError::NotFile);
    }
    if meta.len() > limit {
        return Err(FilesError::TooLarge);
    }
    Ok(fs::read(&file)?)
}

/// What a file's bytes are, as an editor names the version it started
/// from: their SHA-256 in hex.
pub fn version(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Writes through a temporary sibling so a failed write leaves the original
/// intact. With `base` (the [`version`] the editor read), the save is refused
/// as [`FilesError::Changed`] when the file is no longer that version, or is
/// gone, so a change made since the read is never overwritten unseen.
pub fn save(roots: &Roots, path: &str, bytes: &[u8], base: Option<&str>) -> Result<(), FilesError> {
    match roots.existing(path) {
        Ok(file) => {
            let meta = fs::metadata(&file)?;
            if !meta.is_file() {
                return Err(FilesError::NotFile);
            }
            if let Some(base) = base
                && version(&fs::read(&file)?) != base
            {
                return Err(FilesError::Changed);
            }
            // Check the file itself is writable, as an in-place save would.
            drop(fs::OpenOptions::new().write(true).open(&file)?);
            replace(&file, |out| out.write_all(bytes), meta.permissions())?;
            Ok(())
        }
        Err(FilesError::NotFound) if base.is_some() => Err(FilesError::Changed),
        Err(FilesError::NotFound) => {
            let file = roots.entry(path)?;
            let mut out = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)?;
            if let Err(error) = out.write_all(bytes) {
                drop(out);
                let _ = fs::remove_file(&file);
                return Err(error.into());
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// A new file with these bytes, never over anything: written to a
/// temporary sibling and published with a rename that refuses an existing
/// name, so a name taken since the folder was listed is refused, not
/// replaced, and nothing half-written is ever seen under the name.
pub fn upload(roots: &Roots, path: &str, bytes: &[u8]) -> Result<(), FilesError> {
    let file = roots.entry(path)?;
    if fs::symlink_metadata(&file).is_ok() {
        return Err(FilesError::Exists);
    }
    let (temp, mut out) = temporary(&file)?;
    let result = out
        .write_all(bytes)
        .and_then(|()| out.sync_all())
        .and_then(|()| {
            drop(out);
            rename_new(&temp, &file)
        });
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    Ok(result?)
}

/// A new, empty temporary sibling of `file`.
fn temporary(file: &Path) -> std::io::Result<(std::path::PathBuf, fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = file
        .parent()
        .ok_or_else(|| std::io::Error::other("file has no parent"))?;
    loop {
        let temp = parent.join(format!(
            ".parvi-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(out) => return Ok((temp, out)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn replace(
    file: &Path,
    write: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
    permissions: fs::Permissions,
) -> std::io::Result<()> {
    let (temp, mut out) = temporary(file)?;
    let result = out
        .set_permissions(permissions)
        .and_then(|()| write(&mut out));
    drop(out);
    let result = result.and_then(|()| fs::rename(&temp, file));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// A new folder in an existing one; refused when the name is taken.
pub fn create_folder(roots: &Roots, path: &str) -> Result<(), FilesError> {
    Ok(fs::create_dir(roots.entry(path)?)?)
}

/// Renames or moves an entry within the roots, never onto another: the
/// rename itself refuses an existing `to`, so one created after any check
/// is not replaced.
pub fn rename(roots: &Roots, from: &str, to: &str) -> Result<(), FilesError> {
    let from = roots.entry(from)?;
    let to = roots.entry(to)?;
    fs::symlink_metadata(&from)?;
    if fs::symlink_metadata(&to).is_ok() {
        return Err(FilesError::Exists);
    }
    Ok(rename_new(&from, &to)?)
}

/// Renames without replacing a destination created between checking and
/// renaming.
#[cfg(target_os = "linux")]
pub(super) fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from = CString::new(from.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    // SAFETY: both paths are NUL-terminated and remain alive for the call.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// `MoveFileW` without the replace flag refuses an existing destination.
#[cfg(windows)]
pub(super) fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: both strings are NUL-terminated and remain alive for the call.
    let result =
        unsafe { windows_sys::Win32::Storage::FileSystem::MoveFileW(from.as_ptr(), to.as_ptr()) };
    if result != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(super) fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Err(std::io::ErrorKind::AlreadyExists.into());
    }
    fs::rename(from, to)
}

/// Deletes an entry: a folder with everything in it, a link without its
/// target.
pub fn delete(roots: &Roots, path: &str) -> Result<(), FilesError> {
    let entry = roots.entry(path)?;
    if fs::symlink_metadata(&entry)?.is_dir() {
        Ok(fs::remove_dir_all(entry)?)
    } else {
        Ok(fs::remove_file(entry)?)
    }
}

fn modified(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis() as u64)
}

fn text(path: &Path) -> Result<String, FilesError> {
    path.to_str().map(str::to_owned).ok_or(FilesError::Outside)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    use std::path::PathBuf;

    fn setup() -> (Scratch, PathBuf, Roots) {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        let roots = Roots::new(std::slice::from_ref(&root), None).unwrap();
        (scratch, root, roots)
    }

    fn s(path: &Path) -> &str {
        path.to_str().unwrap()
    }

    #[test]
    fn lists_a_folder_by_name() {
        let (_scratch, root, roots) = setup();
        fs::create_dir(root.join("models")).unwrap();
        fs::write(root.join("notes.txt"), "four").unwrap();
        let listing = list(&roots, s(&root)).unwrap();
        assert_eq!(listing.path, s(&root));
        let names: Vec<_> = listing
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.folder, e.size))
            .collect();
        assert_eq!(names, [("models", true, 0), ("notes.txt", false, 4)]);
        assert!(listing.entries[1].modified > 0);
    }

    #[test]
    fn listing_a_file_is_refused() {
        let (_scratch, root, roots) = setup();
        fs::write(root.join("a.txt"), "").unwrap();
        assert!(matches!(
            list(&roots, s(&root.join("a.txt"))),
            Err(FilesError::NotFolder)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_listing_leaves_out_links_out_of_the_roots() {
        let (scratch, root, roots) = setup();
        let secret = scratch.file("secret.txt", "x");
        fs::write(root.join("inside.txt"), "abc").unwrap();
        std::os::unix::fs::symlink(&secret, root.join("out")).unwrap();
        std::os::unix::fs::symlink(root.join("inside.txt"), root.join("in")).unwrap();
        std::os::unix::fs::symlink(root.join("nothing"), root.join("broken")).unwrap();
        let names: Vec<_> = list(&roots, s(&root))
            .unwrap()
            .entries
            .into_iter()
            .map(|e| (e.name, e.size))
            .collect();
        assert_eq!(names, [("in".to_owned(), 3), ("inside.txt".to_owned(), 3)]);
    }

    #[test]
    fn reads_a_file_up_to_the_limit() {
        let (_scratch, root, roots) = setup();
        fs::write(root.join("a.txt"), "hello").unwrap();
        assert_eq!(read(&roots, s(&root.join("a.txt")), 5).unwrap(), b"hello");
        assert!(matches!(
            read(&roots, s(&root.join("a.txt")), 4),
            Err(FilesError::TooLarge)
        ));
        assert!(matches!(
            read(&roots, s(&root), 10),
            Err(FilesError::NotFile)
        ));
        assert!(matches!(
            read(&roots, s(&root.join("gone")), 10),
            Err(FilesError::NotFound)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_save_keeps_the_file_itself() {
        use std::os::unix::fs::PermissionsExt;
        let (_scratch, root, roots) = setup();
        let file = root.join("run.sh");
        fs::write(&file, "old contents that are long").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o750)).unwrap();
        save(&roots, s(&file), b"new", None).unwrap();
        let after = fs::metadata(&file).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"new");
        assert_eq!(after.permissions().mode() & 0o777, 0o750);
    }

    #[test]
    fn failed_write_preserves_original_and_removes_temporary_file() {
        let (_scratch, root, _roots) = setup();
        let file = root.join("a.txt");
        fs::write(&file, "original").unwrap();
        let permissions = fs::metadata(&file).unwrap().permissions();
        let result = replace(
            &file,
            |out| {
                out.write_all(b"partial")?;
                Err(std::io::Error::other("write failed"))
            },
            permissions,
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&file).unwrap(), b"original");
        assert_eq!(fs::read_dir(root).unwrap().count(), 1);
    }

    #[test]
    fn a_save_creates_a_missing_file_but_never_a_folder() {
        let (_scratch, root, roots) = setup();
        save(&roots, s(&root.join("new.md")), b"# hi", None).unwrap();
        assert_eq!(fs::read(root.join("new.md")).unwrap(), b"# hi");
        assert!(matches!(
            save(&roots, s(&root), b"x", None),
            Err(FilesError::NotFile)
        ));
        assert!(matches!(
            save(&roots, s(&root.join("no/such.txt")), b"x", None),
            Err(FilesError::NotFound)
        ));
    }

    #[test]
    fn a_save_from_a_base_refuses_a_file_changed_since() {
        let (_scratch, root, roots) = setup();
        let file = root.join("plan.md");
        fs::write(&file, "read by the editor").unwrap();
        let base = version(b"read by the editor");
        fs::write(&file, "an agent's change").unwrap();
        assert!(matches!(
            save(&roots, s(&file), b"the operator's edit", Some(&base)),
            Err(FilesError::Changed)
        ));
        assert_eq!(fs::read(&file).unwrap(), b"an agent's change");
        save(
            &roots,
            s(&file),
            b"the operator's edit",
            Some(&version(b"an agent's change")),
        )
        .unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"the operator's edit");
        fs::remove_file(&file).unwrap();
        assert!(matches!(
            save(&roots, s(&file), b"x", Some(&base)),
            Err(FilesError::Changed)
        ));
        assert!(!file.exists(), "a file deleted since is not made again");
    }

    #[test]
    fn an_upload_never_replaces_and_leaves_nothing_behind() {
        let (_scratch, root, roots) = setup();
        upload(&roots, s(&root.join("photo.png")), b"new").unwrap();
        assert_eq!(fs::read(root.join("photo.png")).unwrap(), b"new");
        fs::write(root.join("taken.txt"), "keep").unwrap();
        assert!(matches!(
            upload(&roots, s(&root.join("taken.txt")), b"other"),
            Err(FilesError::Exists)
        ));
        assert_eq!(fs::read(root.join("taken.txt")).unwrap(), b"keep");
        let names: Vec<_> = list(&roots, s(&root))
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["photo.png", "taken.txt"], "no temporary left");
    }

    #[test]
    fn the_no_replace_rename_refuses_a_destination_that_appeared() {
        let (_scratch, root, _roots) = setup();
        fs::write(root.join("a.txt"), "a").unwrap();
        // Made after any check a caller could do, as an agent racing the
        // operator would.
        fs::write(root.join("b.txt"), "agent").unwrap();
        let refused = rename_new(&root.join("a.txt"), &root.join("b.txt")).unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(root.join("b.txt")).unwrap(), b"agent");
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"a");
    }

    #[test]
    fn creating_never_replaces() {
        let (_scratch, root, roots) = setup();
        fs::write(root.join("taken.txt"), "keep").unwrap();
        upload(&roots, s(&root.join("empty.txt")), b"").unwrap();
        create_folder(&roots, s(&root.join("dir"))).unwrap();
        assert_eq!(fs::read(root.join("empty.txt")).unwrap(), b"");
        assert!(root.join("dir").is_dir());
        assert!(matches!(
            upload(&roots, s(&root.join("taken.txt")), b""),
            Err(FilesError::Exists)
        ));
        assert!(matches!(
            create_folder(&roots, s(&root.join("taken.txt"))),
            Err(FilesError::Exists)
        ));
        assert_eq!(fs::read(root.join("taken.txt")).unwrap(), b"keep");
    }

    #[test]
    fn rename_moves_to_a_free_name_and_never_clobbers() {
        let (_scratch, root, roots) = setup();
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::write(root.join("b.txt"), "b").unwrap();
        fs::create_dir(root.join("dir")).unwrap();
        assert!(matches!(
            rename(&roots, s(&root.join("a.txt")), s(&root.join("b.txt"))),
            Err(FilesError::Exists)
        ));
        assert!(matches!(
            rename(&roots, s(&root.join("a.txt")), s(&root.join("dir"))),
            Err(FilesError::Exists)
        ));
        assert_eq!(fs::read(root.join("b.txt")).unwrap(), b"b");
        rename(&roots, s(&root.join("a.txt")), s(&root.join("dir/c.txt"))).unwrap();
        assert_eq!(fs::read(root.join("dir/c.txt")).unwrap(), b"a");
        assert!(!root.join("a.txt").exists());
    }

    #[test]
    fn rename_and_delete_refuse_roots_and_outside() {
        let (scratch, root, roots) = setup();
        let outside = scratch.file("outside.txt", "x");
        fs::write(root.join("a.txt"), "a").unwrap();
        assert!(matches!(
            rename(
                &roots,
                s(&root.join("a.txt")),
                s(&outside.with_file_name("moved"))
            ),
            Err(FilesError::Outside)
        ));
        assert!(matches!(
            rename(&roots, s(&outside), s(&root.join("in.txt"))),
            Err(FilesError::Outside)
        ));
        assert!(matches!(delete(&roots, s(&root)), Err(FilesError::Root)));
        assert!(matches!(
            delete(&roots, s(&outside)),
            Err(FilesError::Outside)
        ));
        assert!(outside.exists() && root.exists());
    }

    #[test]
    fn delete_removes_a_folder_with_everything_in_it() {
        let (_scratch, root, roots) = setup();
        fs::create_dir_all(root.join("dir/deep")).unwrap();
        fs::write(root.join("dir/deep/a.txt"), "a").unwrap();
        delete(&roots, s(&root.join("dir"))).unwrap();
        assert!(!root.join("dir").exists());
    }

    #[cfg(unix)]
    #[test]
    fn delete_and_rename_act_on_a_link_not_its_target() {
        let (scratch, root, roots) = setup();
        let target = scratch.dir("target");
        fs::write(target.join("keep.txt"), "keep").unwrap();
        fs::write(root.join("file.txt"), "file").unwrap();
        std::os::unix::fs::symlink(&target, root.join("out")).unwrap();
        std::os::unix::fs::symlink(root.join("file.txt"), root.join("in")).unwrap();
        delete(&roots, s(&root.join("out"))).unwrap();
        assert!(fs::symlink_metadata(root.join("out")).is_err());
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"keep");
        rename(&roots, s(&root.join("in")), s(&root.join("moved"))).unwrap();
        assert!(
            fs::symlink_metadata(root.join("moved"))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"file");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_paths_say_permission() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipping: root reads everything");
            return;
        }
        let (_scratch, root, roots) = setup();
        let locked = root.join("locked");
        fs::create_dir_all(locked.join("inner")).unwrap();
        fs::write(root.join("secret.txt"), "x").unwrap();
        fs::set_permissions(root.join("secret.txt"), fs::Permissions::from_mode(0o000)).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        assert!(matches!(
            list(&roots, s(&locked)),
            Err(FilesError::Permission)
        ));
        assert!(matches!(
            list(&roots, s(&locked.join("inner"))),
            Err(FilesError::Permission)
        ));
        assert!(matches!(
            read(&roots, s(&root.join("secret.txt")), 10),
            Err(FilesError::Permission)
        ));
    }
}
