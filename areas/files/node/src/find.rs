//! Bounded searches under one folder. Links are never traversed.
use super::{error::FilesError, roots::Roots};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const CANDIDATES: usize = 500;
const RESULTS: usize = 50;
const FILE_LIMIT: u64 = 1024 * 1024;
const VISIT_LIMIT: usize = 100_000;

pub use parvi_files_contract::Match;
pub use parvi_files_contract::Matches;

pub fn search(
    roots: &Roots,
    path: &str,
    query: &str,
    content: bool,
) -> Result<Matches, FilesError> {
    let root = roots.existing(path)?;
    if !root.is_dir() {
        return Err(FilesError::NotFolder);
    }
    let needle = query.trim().to_lowercase();
    let (mut found, mut truncated) = if needle.is_empty() {
        (Vec::new(), false)
    } else {
        walk(&root, &needle, content)?
    };
    if !content {
        found.sort_by(|a, b| {
            let a_name = a.name.to_lowercase().contains(&needle);
            let b_name = b.name.to_lowercase().contains(&needle);
            b_name
                .cmp(&a_name)
                .then(a.path.len().cmp(&b.path.len()))
                .then(a.path.cmp(&b.path))
        });
    }
    if found.len() > RESULTS {
        found.truncate(RESULTS);
        truncated = true;
    }
    Ok(Matches {
        matches: found,
        truncated,
    })
}

fn walk(root: &Path, needle: &str, content: bool) -> Result<(Vec<Match>, bool), FilesError> {
    let mounts = mount_points();
    let mut found = Vec::new();
    let mut visited = 0;
    // A failure to read the chosen folder is an error, not an empty result.
    // Inaccessible descendants are still skipped so other matches survive.
    let mut pending = vec![(root.to_path_buf(), Some(fs::read_dir(root)?))];
    while let Some((dir, opened)) = pending.pop() {
        let entries = match opened {
            Some(entries) => entries,
            None => match fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            },
        };
        let mut entries: Vec<_> = entries.flatten().take(VISIT_LIMIT + 1).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries.into_iter().rev() {
            visited += 1;
            if found.len() >= CANDIDATES || visited > VISIT_LIMIT {
                return Ok((found, true));
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if kind.is_dir() {
                if !skipped(&name) && !mounts.contains(&path) {
                    pending.push((path, None));
                }
            } else if kind.is_file() {
                collect(root, path, name, needle, content, &mut found);
            }
        }
    }
    Ok((found, false))
}

/// A folder a walk never enters: hidden, or what a build generates.
pub(super) fn skipped(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules" | "dist" | "build" | "target" | "vendor"
        )
}

/// Linux exposes bind mounts as well as separate filesystems here. Search never
/// enters one beneath its chosen root, as CHROTE's find does.
pub(super) fn mount_points() -> Vec<PathBuf> {
    let Ok(table) = fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    table
        .lines()
        .filter_map(|line| line.split_whitespace().nth(4))
        .map(|path| {
            PathBuf::from(
                path.replace("\\040", " ")
                    .replace("\\011", "\t")
                    .replace("\\012", "\n")
                    .replace("\\134", "\\"),
            )
        })
        .collect()
}

fn collect(
    root: &Path,
    path: PathBuf,
    name: String,
    needle: &str,
    content: bool,
    found: &mut Vec<Match>,
) {
    let Some(path_text) = path.to_str() else {
        return;
    };
    if !content {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if relative.to_lowercase().contains(needle) {
            found.push(Match {
                path: path_text.into(),
                name,
                line: None,
                text: None,
            });
        }
    } else if fs::metadata(&path).is_ok_and(|m| m.len() <= FILE_LIMIT)
        && let Ok(file) = fs::File::open(&path)
        && let Some(text) = read_text(file)
        && !text.contains('\0')
    {
        for (index, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(needle) {
                found.push(Match {
                    path: path_text.into(),
                    name: name.clone(),
                    line: Some(index + 1),
                    text: Some(line.chars().take(500).collect()),
                });
                if found.len() >= CANDIDATES {
                    break;
                }
            }
        }
    }
}

fn read_text(file: fs::File) -> Option<String> {
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > FILE_LIMIT {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;
    #[test]
    fn searches_names_and_contents_without_following_links_or_generated_folders() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        fs::create_dir(root.join("src")).unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(root.join("src/note.txt"), "hello\nWorld\n").unwrap();
        fs::write(root.join("target/note.txt"), "world").unwrap();
        let roots = Roots::new(std::slice::from_ref(&root), None).unwrap();
        let path = root.to_str().unwrap();
        let names = search(&roots, path, "SRC", false).unwrap();
        assert_eq!(names.matches.len(), 1);
        let contents = search(&roots, path, "world", true).unwrap();
        assert_eq!(contents.matches.len(), 1);
        assert_eq!(contents.matches[0].line, Some(2));
    }
    #[test]
    fn limits_and_ranks_results_and_rejects_outside() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        fs::create_dir(root.join("needle-dir")).unwrap();
        fs::write(root.join("needle-dir/long.txt"), "needle\n").unwrap();
        for n in 0..60 {
            fs::write(root.join(format!("needle-{n:02}.txt")), "needle").unwrap();
        }
        let roots = Roots::new(std::slice::from_ref(&root), None).unwrap();
        let result = search(&roots, root.to_str().unwrap(), "needle", false).unwrap();
        assert_eq!(result.matches.len(), 50);
        assert!(result.truncated);
        assert!(result.matches.iter().all(|m| m.name.starts_with("needle-")));
        assert!(matches!(
            search(&roots, scratch.path().to_str().unwrap(), "x", false),
            Err(FilesError::Outside)
        ));
        assert!(
            search(&roots, root.to_str().unwrap(), " ", false)
                .unwrap()
                .matches
                .is_empty()
        );
    }
}
