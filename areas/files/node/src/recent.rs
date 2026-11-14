//! What changed recently, as a person wants to read it: what an agent
//! wrote, not the machine's own state. Files git ignores, lock, pid and
//! socket files and editor leftovers are left out; many files written
//! together (a checkout, a batch of renders) stand as one folder line, and
//! so do more than a few changes in one folder.

use parvi_files_contract::Changed;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The newest files looked at; older ones cannot reach the answer.
pub const CANDIDATES: usize = 4000;
/// Files written within this many milliseconds of the next were written
/// together.
const TOGETHER_MS: u64 = 1500;
/// More files than this in one folder, or written together, stand as one
/// line.
const MANY: usize = 3;

/// A file changed: its path and when.
pub struct File<'a> {
    pub path: &'a str,
    pub modified: u64,
}

/// A name that is the machine's own state, never something to read.
pub fn machine_state(name: &str) -> bool {
    const ENDS: &[&str] = &[
        ".lock",
        ".pid",
        ".sock",
        ".socket",
        ".tmp",
        ".swp",
        ".swo",
        ".part",
        ".crdownload",
        "-journal",
        "-wal",
        "-shm",
        "~",
    ];
    name.starts_with(".#") || ENDS.iter().any(|end| name.ends_with(end))
}

/// The paths among `paths` that their repository ignores, asked of git once
/// per repository. Paths outside a repository are kept.
pub fn ignored(paths: &[&str]) -> std::collections::HashSet<String> {
    let mut repo_of: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    let mut by_repo: HashMap<PathBuf, Vec<&str>> = HashMap::new();
    for path in paths {
        let Some(folder) = Path::new(path).parent() else {
            continue;
        };
        if let Some(repo) = repo(folder, &mut repo_of) {
            by_repo.entry(repo).or_default().push(path);
        }
    }
    let mut out = std::collections::HashSet::new();
    for (repo, paths) in by_repo {
        out.extend(check_ignore(&repo, &paths));
    }
    out
}

/// The repository `folder` is in: the nearest folder above it holding
/// `.git` (a folder, or a worktree's file).
fn repo(folder: &Path, known: &mut HashMap<PathBuf, Option<PathBuf>>) -> Option<PathBuf> {
    if let Some(r) = known.get(folder) {
        return r.clone();
    }
    let found = if folder.join(".git").exists() {
        Some(folder.to_path_buf())
    } else {
        folder.parent().and_then(|up| repo(up, known))
    };
    known.insert(folder.to_path_buf(), found.clone());
    found
}

fn check_ignore(repo: &Path, paths: &[&str]) -> Vec<String> {
    let Ok(mut child) = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["check-ignore", "--stdin", "-z"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return vec![];
    };
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p.as_bytes());
        input.push(0);
    }
    let mut stdin = child.stdin.take().expect("piped");
    // Written beside the read, so a long answer never blocks the question.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let mut answer = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut answer);
    }
    let _ = writer.join();
    let _ = child.wait();
    answer
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .filter_map(|p| std::str::from_utf8(p).ok().map(str::to_owned))
        .collect()
}

/// The lines Recent shows, newest first, at most `most`. `files` come
/// newest first, already rid of what is not to be read; `roots` are the
/// folders a line never rises to.
pub fn summarize(files: &[File], roots: &[String], most: usize) -> Vec<Changed> {
    let mut lines: Vec<Changed> = Vec::new();
    // Files written together, one after another.
    let mut start = 0;
    for i in 1..=files.len() {
        let apart = i == files.len() || files[i - 1].modified - files[i].modified > TOGETHER_MS;
        if apart {
            together(&files[start..i], roots, &mut lines);
            start = i;
        }
    }
    let lines = by_folder(lines);
    let mut merged: Vec<Changed> = Vec::new();
    for line in lines {
        match merged
            .iter_mut()
            .find(|m| m.files.is_some() && line.files.is_some() && m.path == line.path)
        {
            Some(m) => m.files = Some(m.files.unwrap_or(0) + line.files.unwrap_or(0)),
            None => merged.push(line),
        }
    }
    merged.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.path.cmp(&b.path)));
    merged.truncate(most);
    merged
}

/// Files written together: a few stand as themselves; many stand as the
/// folder holding them all, split by the folder under it when that folder
/// would be a root or above one.
fn together(files: &[File], roots: &[String], out: &mut Vec<Changed>) {
    if files.len() <= MANY {
        out.extend(files.iter().map(|f| Changed {
            path: f.path.to_owned(),
            modified: f.modified,
            files: None,
        }));
        return;
    }
    let common = common_folder(files.iter().map(|f| f.path));
    if roots
        .iter()
        .any(|r| Path::new(&common) != Path::new(r) && inside(&common, r))
    {
        out.push(Changed {
            path: common,
            modified: files[0].modified,
            files: Some(files.len()),
        });
        return;
    }
    // At or above a root: one line for each folder under it.
    let mut groups: Vec<(String, Vec<&File>)> = Vec::new();
    for f in files {
        let key = child_under(&common, f.path);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, g)) => g.push(f),
            None => groups.push((key, vec![f])),
        }
    }
    for (_, group) in groups {
        let group: Vec<File> = group
            .into_iter()
            .map(|f| File {
                path: f.path,
                modified: f.modified,
            })
            .collect();
        if group.len() == files.len() {
            // Nothing split them apart: they are files of one folder.
            out.extend(group.iter().map(|f| Changed {
                path: f.path.to_owned(),
                modified: f.modified,
                files: None,
            }));
        } else {
            together(&group, roots, out);
        }
    }
}

/// More than a few files from one folder stand as that folder.
fn by_folder(lines: Vec<Changed>) -> Vec<Changed> {
    let mut count: HashMap<String, usize> = HashMap::new();
    for l in lines.iter().filter(|l| l.files.is_none()) {
        *count.entry(parent(&l.path)).or_default() += 1;
    }
    let mut out: Vec<Changed> = Vec::new();
    for l in lines {
        let folder = parent(&l.path);
        let n = count.get(&folder).copied().unwrap_or(0);
        if l.files.is_none() && n > MANY {
            if let Some(f) = out
                .iter_mut()
                .find(|o| o.files.is_some() && o.path == folder)
            {
                f.modified = f.modified.max(l.modified);
                continue;
            }
            out.push(Changed {
                path: folder,
                modified: l.modified,
                files: Some(n),
            });
        } else {
            out.push(l);
        }
    }
    out
}

fn parent(path: &str) -> String {
    Path::new(path)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or_default()
        .to_owned()
}

fn inside(path: &str, folder: &str) -> bool {
    Path::new(path).starts_with(folder)
}

/// The deepest folder holding every one of `paths`.
fn common_folder<'a>(mut paths: impl Iterator<Item = &'a str>) -> String {
    let Some(first) = paths.next() else {
        return String::new();
    };
    let mut common: PathBuf = Path::new(first).parent().unwrap_or(Path::new("/")).into();
    for p in paths {
        while !Path::new(p).starts_with(&common) {
            if !common.pop() {
                break;
            }
        }
    }
    common.to_string_lossy().into_owned()
}

/// The entry directly under `folder` on the way to `path`.
fn child_under(folder: &str, path: &str) -> String {
    let rest = Path::new(path)
        .strip_prefix(folder)
        .unwrap_or(Path::new(path));
    match rest.components().next() {
        Some(c) => Path::new(folder).join(c).to_string_lossy().into_owned(),
        None => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(files: &[(&str, u64)]) -> Vec<(String, Option<usize>)> {
        let files: Vec<File> = files
            .iter()
            .map(|(path, modified)| File {
                path,
                modified: *modified,
            })
            .collect();
        summarize(&files, &["/r".into()], 40)
            .into_iter()
            .map(|c| (c.path, c.files))
            .collect()
    }

    #[test]
    fn a_checkout_is_one_line_and_an_edit_beside_it_its_own() {
        let mut files: Vec<(String, u64)> = vec![("/r/notes/plan.md".into(), 90_000)];
        for n in 0..200 {
            files.push((format!("/r/wt/src/m{}/f{n}.rs", n % 7), 50_000 - n));
        }
        files.push(("/r/README.md".into(), 10_000));
        let files: Vec<(&str, u64)> = files.iter().map(|(p, m)| (p.as_str(), *m)).collect();
        assert_eq!(
            lines(&files),
            [
                ("/r/notes/plan.md".into(), None),
                ("/r/wt/src".into(), Some(200)),
                ("/r/README.md".into(), None),
            ]
        );
    }

    #[test]
    fn many_changes_in_one_folder_over_time_are_one_line() {
        let files: Vec<(String, u64)> = (0..6)
            .map(|n| (format!("/r/out/{n}.png"), 100_000 - n * 10_000))
            .collect();
        let mut files: Vec<(&str, u64)> = files.iter().map(|(p, m)| (p.as_str(), *m)).collect();
        files.insert(1, ("/r/src/main.rs", 95_000));
        assert_eq!(
            lines(&files),
            [("/r/out".into(), Some(6)), ("/r/src/main.rs".into(), None)]
        );
    }

    #[test]
    fn files_written_together_in_separate_projects_stay_apart() {
        assert_eq!(
            lines(&[
                ("/r/a/x.md", 5_000),
                ("/r/b/y.md", 4_900),
                ("/r/c/z.md", 4_800),
                ("/r/d/w.md", 4_700),
            ]),
            [
                ("/r/a/x.md".into(), None),
                ("/r/b/y.md".into(), None),
                ("/r/c/z.md".into(), None),
                ("/r/d/w.md".into(), None),
            ]
        );
    }

    #[test]
    fn lock_pid_socket_and_editor_files_are_machine_state() {
        for name in [
            "queue.lock",
            "node.pid",
            "tmux.sock",
            ".#notes.md",
            "a.md~",
            "db-wal",
        ] {
            assert!(machine_state(name), "{name}");
        }
        for name in ["plan.md", "render.png", "locker.rs"] {
            assert!(!machine_state(name), "{name}");
        }
    }

    #[test]
    fn what_git_ignores_is_left_out() {
        let scratch = crate::scratch::Scratch::new();
        let repo = scratch.dir("repo");
        let git = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["init", "-q"])
            .status();
        if !git.is_ok_and(|s| s.success()) {
            return;
        }
        std::fs::write(repo.join(".gitignore"), "state/\n*.log\n").unwrap();
        let outside = scratch.dir("plain");
        let paths = [
            format!("{}/state/slot-0", repo.display()),
            format!("{}/run.log", repo.display()),
            format!("{}/plan.md", repo.display()),
            format!("{}/notes.md", outside.display()),
        ];
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let out = ignored(&paths);
        assert!(out.contains(paths[0]) && out.contains(paths[1]), "{out:?}");
        assert!(!out.contains(paths[2]) && !out.contains(paths[3]));
    }
}
