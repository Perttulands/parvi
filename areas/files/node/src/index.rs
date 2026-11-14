//! Every file under the roots, by path and time of change, read by one walk
//! when asked for and kept a little while: what changed most recently, and
//! files by part of their name, answer from it as fast as typing.
//!
//! A walk follows the rules of [`super::find`]: no links, no hidden folders,
//! no generated ones and no other mounted filesystem. Nothing walks on its
//! own: a request builds the index (or refreshes a stale one), and the
//! index is let go once nobody has asked for it a while.
//!
//! A find also takes the folder its asker stands in. Its files come first,
//! and can be asked for alone: that answer never waits for a walk of the
//! roots. When the roots' walk left the folder out (it lies under a hidden
//! or generated one), it is walked for this find alone, so what is beside
//! you is always found.
//!
//! A walk reads folders on several threads: the roots hold hundreds of
//! thousands of files, and one thread took seconds over them.

use super::roots::Roots;
use parvi_files_contract::{Changed, Match, Matches};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The most files one walk takes in.
const MOST: usize = 2_000_000;
/// The most files the walk of the folder a find stands in takes in.
const MOST_NEAR: usize = 100_000;
/// The most threads one walk reads folders on.
const WALKERS: usize = 8;
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

    /// Files whose path holds every word of `query`: those under `near`
    /// first, and those whose name holds the words before the rest. It
    /// looks under every root, and under `near` when the roots' walk leaves
    /// that folder out.
    pub fn find(self: &Arc<Self>, roots: &Roots, query: &str, near: Option<&Path>) -> Matches {
        let Some(words) = words(query) else {
            return nothing();
        };
        let items = self.items(roots, USABLE, true);
        let beside = match near {
            Some(folder) if left_out(roots, folder) => walk_one(folder),
            _ => Vec::new(),
        };
        find_in(beside.iter().chain(items.iter()), &words, near)
    }

    /// The same find over the files under `folder` alone: what
    /// [`Self::find`] puts first, answered without a walk of the roots.
    pub fn find_beside(&self, roots: &Roots, query: &str, folder: &Path) -> Matches {
        let Some(words) = words(query) else {
            return nothing();
        };
        let kept = match self.built.lock().unwrap().as_ref() {
            Some(b) if b.at.elapsed() <= USABLE && !left_out(roots, folder) => {
                Some(b.items.clone())
            }
            _ => None,
        };
        match kept {
            Some(items) => find_in(
                items.iter().filter(|i| under(&i.path, folder)),
                &words,
                None,
            ),
            None => find_in(walk_one(folder).iter(), &words, None),
        }
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
    let starts = roots
        .list()
        .into_iter()
        .map(|r| PathBuf::from(r.path))
        .collect();
    walk_into(starts, MOST)
}

/// The files under the one folder a find stands in.
fn walk_one(folder: &Path) -> Vec<Item> {
    walk_into(vec![folder.to_path_buf()], MOST_NEAR)
}

/// What the threads of one walk share: the folders still to read and how
/// many threads are reading one now, and every folder entered so far (a
/// root inside another root is read once).
struct Walk {
    pending: Mutex<(Vec<PathBuf>, usize)>,
    more: Condvar,
    entered: Mutex<HashSet<PathBuf>>,
    found: AtomicUsize,
    most: usize,
    mounts: Vec<PathBuf>,
}

/// Every file under `starts`, until they are `most`. A start is always
/// entered; under it the walk's rules hold.
fn walk_into(starts: Vec<PathBuf>, most: usize) -> Vec<Item> {
    let walk = Walk {
        pending: Mutex::new((starts, 0)),
        more: Condvar::new(),
        entered: Mutex::new(HashSet::new()),
        found: AtomicUsize::new(0),
        most,
        mounts: super::find::mount_points(),
    };
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(WALKERS));
    std::thread::scope(|scope| {
        let walkers: Vec<_> = (0..threads).map(|_| scope.spawn(|| walk.run())).collect();
        walkers
            .into_iter()
            .flat_map(|w| w.join().expect("a walk thread"))
            .collect()
    })
}

impl Walk {
    /// Reads folders until none is left and no thread may add one.
    fn run(&self) -> Vec<Item> {
        let mut items = Vec::new();
        while let Some(dir) = self.next() {
            let mut below = Vec::new();
            if self.entered.lock().unwrap().insert(dir.clone()) {
                self.read(&dir, &mut items, &mut below);
            }
            let mut pending = self.pending.lock().unwrap();
            pending.0.append(&mut below);
            pending.1 -= 1;
            self.more.notify_all();
        }
        items
    }

    /// A folder to read; none once all are read or the walk is full.
    fn next(&self) -> Option<PathBuf> {
        let mut pending = self.pending.lock().unwrap();
        loop {
            if self.found.load(Ordering::Relaxed) >= self.most {
                pending.0.clear();
            }
            if let Some(dir) = pending.0.pop() {
                pending.1 += 1;
                return Some(dir);
            }
            if pending.1 == 0 {
                return None;
            }
            pending = self.more.wait(pending).unwrap();
        }
    }

    fn read(&self, dir: &Path, items: &mut Vec<Item>, below: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                if !super::find::skipped(&name) && !self.mounts.contains(&path) {
                    below.push(path);
                }
            } else if kind.is_file()
                && let Some(text) = path.to_str()
            {
                if self.found.fetch_add(1, Ordering::Relaxed) >= self.most {
                    return;
                }
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

/// A find's words, lowered; none when it has none.
fn words(query: &str) -> Option<Vec<String>> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    (!words.is_empty()).then_some(words)
}

fn nothing() -> Matches {
    Matches {
        matches: vec![],
        truncated: false,
    }
}

/// Whether the file at `path` lies under `folder`.
fn under(path: &str, folder: &Path) -> bool {
    Path::new(path).starts_with(folder)
}

fn find_in<'a>(
    items: impl Iterator<Item = &'a Item>,
    words: &[String],
    near: Option<&Path>,
) -> Matches {
    // Rank: under the folder stood in, every word in the name, the name
    // starting with the first word, then the shorter path, the newer, and
    // the path itself (a walk lists files in no fixed order).
    let mut hits = Vec::new();
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
        let away = !near.is_some_and(|folder| under(&item.path, folder));
        let key = (
            away,
            rank,
            item.path.len(),
            u64::MAX - item.modified,
            item.path.as_str(),
        );
        hits.push((key, item));
    }
    let truncated = hits.len() > RESULTS;
    if truncated {
        hits.select_nth_unstable_by_key(RESULTS, |hit| hit.0);
        hits.truncate(RESULTS);
    }
    hits.sort_unstable_by_key(|hit| hit.0);
    Matches {
        matches: hits
            .into_iter()
            .map(|(_, item)| Match {
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
            find_in(items.iter(), &words, None)
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
    fn what_is_under_the_folder_stood_in_comes_first() {
        let items = [
            item("/r/terrain.rs", 1),
            item("/r/here/deep/old_terrain.rs", 1),
            item("/r/here/terrain_notes.md", 1),
            item("/r/hereafter/terrain.rs", 1),
        ];
        let words = ["terrain".to_string()];
        let paths = |near: Option<&Path>| -> Vec<String> {
            find_in(items.iter(), &words, near)
                .matches
                .into_iter()
                .map(|m| m.path)
                .collect()
        };
        assert_eq!(paths(None)[0], "/r/terrain.rs");
        assert_eq!(
            paths(Some(Path::new("/r/here"))),
            [
                "/r/here/terrain_notes.md",
                "/r/here/deep/old_terrain.rs",
                "/r/terrain.rs",
                "/r/hereafter/terrain.rs"
            ],
            "its own files, best first, then the rest; a folder named alike is not it"
        );
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
                "/.cache/walk/work/pic.png",
                "/.cache/walk/work/deep/pic-two.png",
                "/src/pic.rs"
            ]
        );
        // Asked for alone, what is beside you is the head of that answer,
        // whether the roots' walk covers the folder or not.
        let beside = |folder: &Path| -> Vec<String> {
            index
                .find_beside(&roots, "pic", folder)
                .matches
                .into_iter()
                .map(|m| m.name)
                .collect()
        };
        assert_eq!(beside(&here), ["pic.png", "pic-two.png"]);
        assert_eq!(beside(&root.join("src")), ["pic.rs"]);
        assert_eq!(
            Index::default()
                .find_beside(&roots, "pic", &root.join("src"))
                .matches
                .len(),
            1,
            "with no index kept, the folder is walked"
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
