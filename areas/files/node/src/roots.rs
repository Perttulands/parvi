//! The folders a node shows, and the one rule every path obeys: it resolves
//! to somewhere under one of them.
//!
//! Reading follows symbolic links, so a link inside a root that points out of
//! every root reads as outside. Changing a path resolves only its parent: a
//! rename or delete of a link acts on the link, never on what it points to,
//! and a parent that is a link out of the roots is caught.

use std::io;
use std::path::{Component, Path, PathBuf};

use super::error::FilesError;

/// One configured root: its canonical path and the word it shows as.
pub use parvi_files_contract::Root;

#[derive(Debug, Clone)]
pub struct Roots {
    roots: Vec<(Root, PathBuf)>,
}

impl Roots {
    /// Canonicalises each root, keeping the first of any repeated. A root that
    /// is the home folder shows as `~`. A missing root is an error: it was
    /// named on purpose.
    pub fn new(paths: &[PathBuf], home: Option<&Path>) -> io::Result<Roots> {
        let home = home.and_then(|home| home.canonicalize().ok());
        let mut roots: Vec<(Root, PathBuf)> = Vec::new();
        for path in paths {
            let canonical = path.canonicalize().map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("files root {}: {error}", path.display()),
                )
            })?;
            if roots.iter().any(|(_, p)| *p == canonical) {
                continue;
            }
            let Some(text) = canonical.to_str() else {
                return Err(io::Error::other(format!(
                    "files root {} is not UTF-8",
                    canonical.display()
                )));
            };
            let name = if home.as_deref() == Some(canonical.as_path()) {
                "~".to_owned()
            } else {
                label(text)
            };
            let root = Root {
                name,
                path: text.to_owned(),
            };
            roots.push((root, canonical));
        }
        Ok(Roots { roots })
    }

    pub fn list(&self) -> Vec<Root> {
        self.roots.iter().map(|(root, _)| root.clone()).collect()
    }

    /// An existing path under a root, with every link resolved. A root itself
    /// is one.
    pub fn existing(&self, path: &str) -> Result<PathBuf, FilesError> {
        let path = absolute(path)?;
        let canonical = path.canonicalize()?;
        if self.contains(&canonical) {
            Ok(canonical)
        } else {
            Err(FilesError::Outside)
        }
    }

    /// The entry a path names, existing or not: its parent resolved, its last
    /// component as given. Never a root itself.
    pub fn entry(&self, path: &str) -> Result<PathBuf, FilesError> {
        let path = absolute(path)?;
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(FilesError::Root);
        };
        let entry = parent.canonicalize()?.join(name);
        if self.roots.iter().any(|(_, root)| *root == entry) {
            return Err(FilesError::Root);
        }
        if self.contains(&entry) {
            Ok(entry)
        } else {
            Err(FilesError::Outside)
        }
    }

    pub fn contains(&self, canonical: &Path) -> bool {
        self.roots
            .iter()
            .any(|(_, root)| canonical.starts_with(root))
    }
}

/// How a root reads: its path without Windows' verbatim prefix, which
/// canonicalising adds (`\\?\C:\work` reads `C:\work`, `\\?\UNC\host\share`
/// reads `\\host\share`). Only the name changes; the path stays verbatim.
fn label(path: &str) -> String {
    if let Some(share) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{share}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
    }
}

/// An absolute path whose last component is a name, not `.` or `..`.
fn absolute(path: &str) -> Result<&Path, FilesError> {
    // Windows does not normalize dot components under a verbatim prefix.
    // Check the original spelling: Path::components can discard `.` and
    // verbatim paths may contain either separator in client input.
    #[cfg(windows)]
    if path.starts_with(r"\\?\")
        && path
            .split(['\\', '/'])
            .any(|part| part == "." || part == "..")
    {
        return Err(FilesError::Outside);
    }
    let path = Path::new(path);
    if !path.is_absolute() || path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(FilesError::Outside);
    }
    match path.components().next_back() {
        Some(Component::CurDir | Component::ParentDir) => Err(FilesError::Outside),
        _ => Ok(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    #[test]
    fn a_root_reads_without_the_verbatim_prefix() {
        assert_eq!(label(r"\\?\C:\Users\me\work"), r"C:\Users\me\work");
        assert_eq!(label(r"\\?\UNC\host\share\x"), r"\\host\share\x");
        assert_eq!(label("/home/me/work"), "/home/me/work");
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_dot_components_cannot_escape_roots() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        scratch.file("outside.txt", "secret");
        let roots = Roots::new(std::slice::from_ref(&root), None).unwrap();
        let root = root.to_str().unwrap();
        assert!(root.starts_with(r"\\?\"));

        for suffix in [
            r"\..\outside.txt",
            r"\..\missing.txt",
            r"\..\root\inside.txt",
            r"\.\inside.txt",
            r"/../outside.txt",
            r"\../missing.txt",
            r"/./inside.txt",
            r"\..",
            r"\.",
        ] {
            let path = format!("{root}{suffix}");
            assert!(
                matches!(roots.existing(&path), Err(FilesError::Outside)),
                "existing: {path}"
            );
            assert!(
                matches!(roots.entry(&path), Err(FilesError::Outside)),
                "entry: {path}"
            );
        }
    }

    #[test]
    fn names_the_home_root_with_a_tilde() {
        let scratch = Scratch::new();
        let other = scratch.dir("other");
        let roots = Roots::new(&[scratch.path(), other.clone()], Some(&scratch.path())).unwrap();
        let names: Vec<String> = roots.list().into_iter().map(|r| r.name).collect();
        // The name drops Windows' verbatim prefix; the path keeps it.
        let other = other.to_str().unwrap();
        let shown = other.strip_prefix(r"\\?\").unwrap_or(other);
        assert_eq!(names, ["~".to_owned(), shown.to_owned()]);
    }

    #[test]
    fn refuses_a_missing_root() {
        let scratch = Scratch::new();
        assert!(Roots::new(&[scratch.path().join("gone")], None).is_err());
    }

    #[test]
    fn refuses_relative_and_outside_paths() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        scratch.file("outside.txt", "x");
        let roots = Roots::new(std::slice::from_ref(&root), None).unwrap();
        let outside = scratch.path().join("outside.txt");
        assert!(matches!(
            roots.existing("relative"),
            Err(FilesError::Outside)
        ));
        assert!(matches!(
            roots.existing(outside.to_str().unwrap()),
            Err(FilesError::Outside)
        ));
        // Use a native, non-verbatim path: Windows canonical paths have a
        // verbatim prefix, under which the OS does not interpret `..`.
        let native_root = std::env::temp_dir()
            .join(scratch.path().file_name().unwrap())
            .join("root");
        let climb = native_root.join("..").join("outside.txt");
        let climb = climb.to_str().unwrap();
        assert!(matches!(roots.existing(climb), Err(FilesError::Outside)));
        assert!(matches!(roots.entry(climb), Err(FilesError::Outside)));
    }

    #[test]
    fn a_root_itself_reads_but_never_changes() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        let roots = Roots::new(std::slice::from_ref(&root), None).unwrap();
        let text = root.to_str().unwrap();
        assert_eq!(roots.existing(text).unwrap(), root.canonicalize().unwrap());
        assert!(matches!(roots.entry(text), Err(FilesError::Root)));
        let filesystem_root = root.ancestors().last().unwrap();
        assert!(matches!(
            roots.entry(filesystem_root.to_str().unwrap()),
            Err(FilesError::Root)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_cannot_escape_for_reading() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        scratch.file("secret.txt", "x");
        std::os::unix::fs::symlink(scratch.path().join("secret.txt"), root.join("link")).unwrap();
        let roots = Roots::new(std::slice::from_ref(&root), None).unwrap();
        let link = root.join("link");
        assert!(matches!(
            roots.existing(link.to_str().unwrap()),
            Err(FilesError::Outside)
        ));
        // Changing the link itself is allowed: it lives in the root.
        assert_eq!(roots.entry(link.to_str().unwrap()).unwrap(), link);
    }

    #[cfg(unix)]
    #[test]
    fn a_parent_link_cannot_escape_for_changing() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        let outside = scratch.dir("outside");
        std::os::unix::fs::symlink(&outside, root.join("door")).unwrap();
        let roots = Roots::new(std::slice::from_ref(&root), None).unwrap();
        let through = root.join("door").join("new.txt");
        assert!(matches!(
            roots.entry(through.to_str().unwrap()),
            Err(FilesError::Outside)
        ));
    }
}
