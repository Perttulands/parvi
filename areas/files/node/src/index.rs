//! Every file under the roots, by path and time of change, read by one walk
//! when asked for and kept a little while: what changed most recently, and
//! files by part of their name, answer from it as fast as typing.
//!
//! A walk follows the rules of [`super::find`]: no links, no hidden folders,
//! no generated ones and no other mounted filesystem. Nothing walks on its
//! own: a request builds the index (or refreshes a stale one), and the
//! index is let go once nobody has asked for it a while.
//!
//! A find also takes the folder its asker stands in. When the walk of the
//! roots left that folder out (it lies under a hidden or generated one),
//! it is walked for this find alone, so what is beside you is always found.

use super::roots::Roots;
use parvi_files_contract::{Changed, Match, Matches};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The most files one walk takes in.
const MOST: usize = 2_000_000;
/// The most files the walk of the folder a find stands in takes in.
const MOST_NEAR: usize = 100_000;
/// Results a name find answers.
const RESULTS: usize = 50;
/// What changed answers this many lines at most.
pub const RECENT: usize = 40;
/// How old an index a recent-changes answer accepts, and a name find (a
/// find answers from an older one and refreshes it behind).
const FRESH: Duration = Duration::from_secs(2);
const USABLE: Duration = Duration::from_secs(60);
/// Unused this long, the index is let go.
const IDLE: Duration = Duration::from_secs(120);

#[derive(Debug)]
struct Item {
    path: String,
    /// Where the name starts in `path`.
    name: usize,
    modified: u64,
}

struct Built {
    at: Instant,
    items: Arc<Vec<Item>>,
}

#[derive(Default)]
pub struct Index {
    built: Mutex<Option<Built>>,
    /// Held while a walk runs, so two requests never walk at once.
    walking: Mutex<()>,
    used: Mutex<Option<Instant>>,
}

impl Index {
    /// What changed most recently, newest first, as [`super::recent`]
    /// sums it up.
    pub fn recent(self: &Arc<Self>, roots: &Roots) -> Vec<Changed> {
        let items = self.items(roots, FRESH, false);
        recent_of(&items, roots, RECENT)
    }

    /// Files whose path holds every word of `query`, those whose name does
    /// first: under every root, and under `near` when the roots' walk
    /// leaves that folder out.
    pub fn find(self: &Arc<Self>, roots: &Roots, query: &str, near: Option<&Path>) -> Matches {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Matches {
                matches: vec![],
                truncated: false,
            };
        }
        let items = self.items(roots, USABLE, true);
        let beside = match near {
            Some(folder) if left_out(roots, folder) => {
                let mut items = Vec::new();
                walk_into(
                    folder.to_path_buf(),
                    &super::find::mount_points(),
                    &mut HashSet::new(),
                    &mut items,
                    MOST_NEAR,
                );
                items
            }
            _ => Vec::new(),
        };
        find_in(beside.iter().chain(items.iter()), &words)
    }

    /// The files changed most recently under `folder`, newest first, each
    /// as itself: what one line of [`Self::recent`] stands for.
    pub fn recent_under(self: &Arc<Self>, roots: &Roots, folder: &Path) -> Vec<Changed> {
        let items = self.items(roots, FRESH, false);
        let mut inside: Vec<&Item> = items
            .iter()
            .filter(|i| Path::new(&i.path).starts_with(folder))
            .filter(|i| !super::recent::machine_state(&i.path[i.name..]))
            .collect();
        inside.sort_unstable_by(|a, b| b.modified.cmp(&a.modified).then(a.path.cmp(&b.path)));
        inside.truncate(super::recent::CANDIDATES);
        let paths: Vec<&str> = inside.iter().map(|i| i.path.as_str()).collect();
        let ignored = super::recent::ignored(&paths);
        inside
            .into_iter()
            .filter(|i| !ignored.contains(&i.path))
            .take(RECENT)
            .map(|i| Changed {
                path: i.path.clone(),
                modified: i.modified,
                files: None,
            })
            .collect()
    }

    fn items(self: &Arc<Self>, roots: &Roots, age: Duration, behind: bool) -> Arc<Vec<Item>> {
        *self.used.lock().unwrap() = Some(Instant::now());
        let stale = {
            let built = self.built.lock().unwrap();
            match built.as_ref() {
                Some(b) if b.at.elapsed() <= FRESH.min(age) => return b.items.clone(),
                Some(b) if behind && b.at.elapsed() <= age => Some(b.items.clone()),
                _ => None,
            }
        };
        if let Some(items) = stale {
            // Answer now; the next question gets a fresh walk.
            let me = self.clone();
            let roots = roots.clone();
            std::thread::spawn(move || me.walk(&roots, FRESH));
            return items;
        }
        self.walk(roots, age)
    }

    /// Walks unless a walk that just ended answers for it.
    fn walk(self: &Arc<Self>, roots: &Roots, age: Duration) -> Arc<Vec<Item>> {
        let _one = self.walking.lock().unwrap();
        if let Some(b) = self.built.lock().unwrap().as_ref()
            && b.at.elapsed() <= age.min(FRESH)
        {
            return b.items.clone();
        }
        let items = Arc::new(walk(roots));
        let first = self
            .built
            .lock()
            .unwrap()
            .replace(Built {
                at: Instant::now(),
                items: items.clone(),
            })
            .is_none();
        if first {
            let me = Arc::downgrade(self);
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(IDLE);
                    let Some(me) = me.upgrade() else { return };
                    let used = me.used.lock().unwrap().is_none_or(|u| u.elapsed() >= IDLE);
                    if used {
                        *me.built.lock().unwrap() = None;
                        return;
                    }
                }
            });
        }
        items
    }
}

fn walk(roots: &Roots) -> Vec<Item> {
    let mounts = super::find::mount_points();
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for root in roots.list() {
        walk_into(
            PathBuf::from(&root.path),
            &mounts,
            &mut seen,
            &mut items,
            MOST,
        );
    }
    items
}

/// Every file under `start` joins `items`, until they are `most`. `start`
/// itself is always entered; under it the walk's rules hold.
fn walk_into(
    start: PathBuf,
    mounts: &[PathBuf],
    seen: &mut HashSet<String>,
    items: &mut Vec<Item>,
    most: usize,
) {
    let mut pending = vec![start];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if items.len() >= most {
                return;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                if !super::find::skipped(&name) && !mounts.contains(&path) {
                    pending.push(path);
                }
            } else if kind.is_file()
                && let Some(text) = path.to_str()
                && seen.insert(text.to_owned())
            {
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as u64);
                items.push(Item {
                    name: text.len() - name.len(),
                    path: text.to_owned(),
                    modified,
                });
            }
        }
    }
}

/// Whether the walk of the roots never enters `folder` (canonical, under a
/// root): a folder on the way down to it is skipped or is another mount.
fn left_out(roots: &Roots, folder: &Path) -> bool {
    let mounts = super::find::mount_points();
    let Some(root) = roots
        .list()
        .into_iter()
        .map(|r| PathBuf::from(r.path))
        .filter(|r| folder.starts_with(r))
        .max_by_key(|r| r.as_os_str().len())
    else {
        return false;
    };
    let Ok(below) = folder.strip_prefix(&root) else {
        return false;
    };
    let mut at = root;
    below.components().any(|part| {
        at.push(part);
        part.as_os_str().to_str().is_none_or(super::find::skipped) || mounts.contains(&at)
    })
}

fn recent_of(items: &[Item], roots: &Roots, most: usize) -> Vec<Changed> {
    use super::recent;
    let mut order: Vec<&Item> = items
        .iter()
        .filter(|i| !recent::machine_state(&i.path[i.name..]))
        .collect();
    order.sort_unstable_by(|a, b| b.modified.cmp(&a.modified).then(a.path.cmp(&b.path)));
    order.truncate(recent::CANDIDATES);
    let paths: Vec<&str> = order.iter().map(|i| i.path.as_str()).collect();
    let ignored = recent::ignored(&paths);
    let files: Vec<recent::File> = order
        .iter()
        .filter(|i| !ignored.contains(&i.path))
        .map(|i| recent::File {
            path: &i.path,
            modified: i.modified,
        })
        .collect();
    let roots: Vec<String> = roots.list().into_iter().map(|r| r.path).collect();
    recent::summarize(&files, &roots, most)
}

fn find_in<'a>(items: impl Iterator<Item = &'a Item>, words: &[String]) -> Matches {
    // Rank: every word in the name, the name starting with the first word,
    // then the shorter path, then the newer.
    let mut hits: Vec<(u8, usize, u64, &Item)> = Vec::new();
    for item in items {
        let path = item.path.to_lowercase();
        if !words.iter().all(|w| path.contains(w.as_str())) {
            continue;
        }
        let name = &path[item.name..];
        let rank = if name.starts_with(words[0].as_str()) {
            0
        } else if words.iter().all(|w| name.contains(w.as_str())) {
            1
        } else if words.iter().any(|w| name.contains(w.as_str())) {
            2
        } else {
            3
        };
        hits.push((rank, item.path.len(), u64::MAX - item.modified, item));
    }
    let truncated = hits.len() > RESULTS;
    if truncated {
        hits.select_nth_unstable_by(RESULTS, |a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
        hits.truncate(RESULTS);
    }
    hits.sort_by_key(|a| (a.0, a.1, a.2));
    Matches {
        matches: hits
            .into_iter()
            .map(|(_, _, _, item)| Match {
                path: item.path.clone(),
                name: item.path[item.name..].to_owned(),
                line: None,
                text: None,
            })
            .collect(),
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    fn item(path: &str, modified: u64) -> Item {
        Item {
            name: path.rfind('/').map_or(0, |i| i + 1),
            path: path.into(),
            modified,
        }
    }

    #[test]
    fn find_takes_every_word_and_ranks_names_first() {
        let items = [
            item("/r/terrain/notes.md", 1),
            item("/r/src/world/terrain_generator.rs", 1),
            item("/r/src/terrain.rs", 1),
            item("/r/docs/generator.md", 1),
        ];
        let found = |q: &str| -> Vec<String> {
            let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
            find_in(items.iter(), &words)
                .matches
                .into_iter()
                .map(|m| m.path)
                .collect()
        };
        assert_eq!(
            found("terrain"),
            [
                "/r/src/terrain.rs",
                "/r/src/world/terrain_generator.rs",
                "/r/terrain/notes.md"
            ]
        );
        assert_eq!(found("Gen world"), ["/r/src/world/terrain_generator.rs"]);
        assert!(found("nothing").is_empty());
    }

    #[test]
    fn a_walk_skips_hidden_and_generated_folders_and_counts_a_file_once() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("src/a.rs"), "a").unwrap();
        fs::write(root.join(".git/HEAD"), "x").unwrap();
        fs::write(root.join("target/b.rs"), "b").unwrap();
        let roots = Roots::new(&[root.clone(), root.join("src")], None).unwrap();
        let index = Arc::new(Index::default());
        let recent = index.recent(&roots);
        assert_eq!(recent.len(), 1);
        assert!(recent[0].path.ends_with("/src/a.rs"));
        let found = index.find(&roots, "a.rs", None);
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].name, "a.rs");
    }

    #[test]
    fn a_find_covers_the_folder_it_stands_in_under_a_hidden_one() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        let here = root.join(".cache/walk/work");
        fs::create_dir_all(here.join("deep")).unwrap();
        fs::create_dir_all(root.join(".cache/other")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(here.join("pic.png"), "p").unwrap();
        fs::write(here.join("deep/pic-two.png"), "p").unwrap();
        fs::write(root.join(".cache/other/pic.png"), "p").unwrap();
        fs::write(root.join("src/pic.rs"), "p").unwrap();
        let roots = Roots::new(std::slice::from_ref(&root), None).unwrap();
        let index = Arc::new(Index::default());
        let names = |near: Option<&Path>, q: &str| -> Vec<String> {
            let root = root.to_str().unwrap();
            index
                .find(&roots, q, near)
                .matches
                .into_iter()
                .map(|m| m.path[root.len()..].replace('\\', "/"))
                .collect()
        };
        // From nowhere the hidden folder stays out of the broad walk.
        assert_eq!(names(None, "pic"), ["/src/pic.rs"]);
        // Standing in it, its files are found, by name or by words of the
        // path; its hidden neighbour still is not.
        assert_eq!(
            names(Some(&here), "pic"),
            [
                "/src/pic.rs",
                "/.cache/walk/work/pic.png",
                "/.cache/walk/work/deep/pic-two.png"
            ]
        );
        assert_eq!(
            names(Some(&here), "walk pic.png"),
            ["/.cache/walk/work/pic.png"]
        );
        // A folder the broad walk covers is not walked twice.
        assert_eq!(names(Some(&root.join("src")), "pic"), ["/src/pic.rs"]);
    }

    #[test]
    fn what_changed_under_a_folder_lists_its_files_newest_first() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::create_dir_all(root.join("c")).unwrap();
        for (path, at) in [("a/one.md", 10), ("a/b/two.md", 30), ("c/three.md", 20)] {
            let file = fs::File::create(root.join(path)).unwrap();
            file.set_modified(UNIX_EPOCH + Duration::from_secs(at))
                .unwrap();
        }
        fs::write(root.join("a/x.lock"), "x").unwrap();
        let roots = Roots::new(std::slice::from_ref(&root), None).unwrap();
        let index = Arc::new(Index::default());
        let under = index.recent_under(&roots, &root.join("a"));
        let names: Vec<&str> = under
            .iter()
            .map(|c| c.path.rsplit(['/', '\\']).next().unwrap())
            .collect();
        assert_eq!(names, ["two.md", "one.md"]);
    }
}
