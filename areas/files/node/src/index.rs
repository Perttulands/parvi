//! Every file under the roots, by path and time of change, read by one walk
//! when asked for and kept a little while: what changed most recently, and
//! files by part of their name, answer from it as fast as typing.
//!
//! A walk follows the rules of [`super::find`]: no links, no hidden folders,
//! no generated ones and no other mounted filesystem. Nothing walks on its
//! own: a request builds the index (or refreshes a stale one), and the
//! index is let go once nobody has asked for it a while.

use super::roots::Roots;
use parvi_files_contract::{Changed, Match, Matches};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The most files one walk takes in.
const MOST: usize = 2_000_000;
/// Results a name find answers.
const RESULTS: usize = 50;
/// What changed answers this many at most, and at most `PER_FOLDER` from
/// one folder, so one agent writing a hundred files leaves room for others.
pub const RECENT: usize = 40;
const PER_FOLDER: usize = 3;
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
    /// The files changed most recently, newest first.
    pub fn recent(self: &Arc<Self>, roots: &Roots) -> Vec<Changed> {
        let items = self.items(roots, FRESH, false);
        recent_of(&items, RECENT)
    }

    /// Files whose path holds every word of `query`, those whose name does
    /// first.
    pub fn find(self: &Arc<Self>, roots: &Roots, query: &str) -> Matches {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Matches {
                matches: vec![],
                truncated: false,
            };
        }
        let items = self.items(roots, USABLE, true);
        find_in(&items, &words)
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
        let mut pending = vec![PathBuf::from(&root.path)];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if items.len() >= MOST {
                    return items;
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
    items
}

fn folder_of(path: &str) -> &str {
    Path::new(path)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or_default()
}

fn recent_of(items: &[Item], most: usize) -> Vec<Changed> {
    let mut order: Vec<&Item> = items.iter().collect();
    order.sort_unstable_by(|a, b| b.modified.cmp(&a.modified).then(a.path.cmp(&b.path)));
    let mut per_folder: std::collections::HashMap<&str, usize> = Default::default();
    let mut out = Vec::new();
    for item in order {
        let n = per_folder.entry(folder_of(&item.path)).or_default();
        if *n >= PER_FOLDER {
            continue;
        }
        *n += 1;
        out.push(Changed {
            path: item.path.clone(),
            modified: item.modified,
        });
        if out.len() >= most {
            break;
        }
    }
    out
}

fn find_in(items: &[Item], words: &[String]) -> Matches {
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
    fn recent_changes_come_newest_first_a_few_per_folder() {
        let mut items: Vec<Item> = (0..10)
            .map(|n| item(&format!("/r/out/{n}.png"), 100 + n))
            .collect();
        items.push(item("/r/src/main.rs", 50));
        items.push(item("/r/README.md", 200));
        let recent = recent_of(&items, 40);
        let paths: Vec<&str> = recent.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/r/README.md",
                "/r/out/9.png",
                "/r/out/8.png",
                "/r/out/7.png",
                "/r/src/main.rs"
            ]
        );
    }

    #[test]
    fn find_takes_every_word_and_ranks_names_first() {
        let items = vec![
            item("/r/terrain/notes.md", 1),
            item("/r/src/world/terrain_generator.rs", 1),
            item("/r/src/terrain.rs", 1),
            item("/r/docs/generator.md", 1),
        ];
        let found = |q: &str| -> Vec<String> {
            let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
            find_in(&items, &words)
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
        let found = index.find(&roots, "a.rs");
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].name, "a.rs");
    }
}
