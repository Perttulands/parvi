//! The folder tree of one device: its roots, the folders opened, what each
//! holds as last read, and the row picked. No drawing and no fetching: the
//! page asks, then hands the answers here.

use super::listing;
use super::paths;
use parvi_files_contract::{Entry, Root};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

/// Where a tree was, kept across restarts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    /// The node's id; empty means the serving node.
    pub node: String,
    pub open: Vec<String>,
    pub selected: Option<String>,
}

/// One row of the tree as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub folder: bool,
    pub open: bool,
    /// A folder opened whose contents are still being read.
    pub reading: bool,
    pub size: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub node: String,
    pub roots: Vec<Root>,
    pub open: BTreeSet<String>,
    pub listings: HashMap<String, Vec<Entry>>,
    /// Folders that could not be read, and why.
    pub failed: HashMap<String, String>,
    pub selected: Option<String>,
    /// The device is away: what is shown is the last read, and nothing that
    /// writes is offered.
    pub away: bool,
}

impl Tree {
    pub fn from_kept(k: &Kept) -> Self {
        Self {
            node: k.node.clone(),
            open: k.open.iter().cloned().collect(),
            selected: k.selected.clone(),
            ..Default::default()
        }
    }

    pub fn kept(&self) -> Kept {
        Kept {
            node: self.node.clone(),
            open: self.open.iter().cloned().collect(),
            selected: self.selected.clone(),
        }
    }

    /// A new device: what was shown belongs to the old one.
    pub fn choose_node(&mut self, node: &str) {
        if self.node != node {
            *self = Tree {
                node: node.to_string(),
                ..Default::default()
            };
        }
    }

    /// The roots arrived. Folders kept open outside them close; while no
    /// root is open the first one opens.
    pub fn rooted(&mut self, roots: Vec<Root>) {
        self.roots = roots;
        let roots = &self.roots;
        self.open.retain(|p| paths::root_of(roots, p).is_some());
        if !self.roots.iter().any(|r| self.open.contains(&r.path))
            && let Some(first) = self.roots.first()
        {
            self.open.insert(first.path.clone());
        }
    }

    /// Open folders whose contents are not read yet.
    pub fn unread(&self) -> Vec<String> {
        self.rows()
            .into_iter()
            .filter(|r| r.reading)
            .map(|r| r.path)
            .collect()
    }

    /// The rows in order: each root, and inside every open folder what it
    /// holds.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for root in &self.roots {
            self.push(&mut rows, root.path.clone(), root.name.clone(), 0, true, 0);
        }
        rows
    }

    fn push(
        &self,
        rows: &mut Vec<Row>,
        path: String,
        name: String,
        depth: usize,
        folder: bool,
        size: u64,
    ) {
        let open = folder && self.open.contains(&path);
        let entries = self.listings.get(&path);
        rows.push(Row {
            path: path.clone(),
            name,
            depth,
            folder,
            open,
            reading: open && entries.is_none() && !self.failed.contains_key(&path),
            size,
        });
        if let (true, Some(entries)) = (open, entries) {
            for e in listing::shown(entries) {
                self.push(
                    rows,
                    paths::join(&path, &e.name),
                    e.name.clone(),
                    depth + 1,
                    e.folder,
                    e.size,
                );
            }
        }
    }

    pub fn row(&self, path: &str) -> Option<Row> {
        self.rows().into_iter().find(|r| r.path == path)
    }

    /// A folder's contents arrived.
    pub fn listed(&mut self, path: String, entries: Vec<Entry>) {
        self.failed.remove(&path);
        self.away = false;
        self.listings.insert(path, entries);
    }

    pub fn failed(&mut self, path: String, why: String) {
        self.failed.insert(path, why);
    }

    /// Opens or closes a folder; true when it opened and needs reading.
    pub fn toggle(&mut self, path: &str) -> bool {
        if self.open.remove(path) {
            return false;
        }
        self.open.insert(path.to_string());
        !self.listings.contains_key(path)
    }

    /// Opens every folder from the root down to `path`'s, so its row
    /// shows; answers those still to read.
    pub fn reveal(&mut self, path: &str) -> Vec<String> {
        let Some(root) = paths::root_of(&self.roots, path) else {
            return vec![];
        };
        let crumbs = paths::crumbs(root, &paths::parent(path));
        let mut unread = Vec::new();
        for crumb in crumbs {
            if !paths::inside(&crumb.path, path) || crumb.path == path {
                continue;
            }
            self.open.insert(crumb.path.clone());
            if !self.listings.contains_key(&crumb.path) {
                unread.push(crumb.path);
            }
        }
        unread
    }

    pub fn select(&mut self, path: &str) {
        self.selected = Some(path.to_string());
    }

    /// The picked row while it shows.
    pub fn selection(&self) -> Option<Row> {
        self.row(self.selected.as_deref()?)
    }

    /// Moves the pick by `by` rows; with nothing picked the first (or
    /// last) row.
    pub fn step(&mut self, by: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|s| rows.iter().position(|r| &r.path == s));
        let next = match at {
            Some(i) => (i as isize)
                .saturating_add(by)
                .clamp(0, rows.len() as isize - 1) as usize,
            None if by < 0 => rows.len() - 1,
            None => 0,
        };
        self.selected = Some(rows[next].path.clone());
    }

    /// Left: an open folder closes, anything else picks its folder.
    pub fn left(&mut self) {
        let Some(row) = self.selection() else { return };
        if row.open {
            self.open.remove(&row.path);
        } else if row.depth > 0 {
            self.selected = Some(paths::parent(&row.path));
        }
    }

    /// Right: a closed folder opens (true when it needs reading), an open
    /// one picks its first row.
    pub fn right(&mut self) -> bool {
        let Some(row) = self.selection().filter(|r| r.folder) else {
            return false;
        };
        if !row.open {
            return self.toggle(&row.path);
        }
        self.step(1);
        false
    }

    /// The folder a new file, a new folder or an upload goes in: the picked
    /// folder, or the picked file's, or the first root.
    pub fn folder_here(&self) -> Option<String> {
        match self.selection() {
            Some(r) if r.folder => Some(r.path),
            Some(r) => Some(paths::parent(&r.path)),
            None => self.roots.first().map(|r| r.path.clone()),
        }
    }

    /// Whether writes are offered: the device answers.
    pub fn writable(&self) -> bool {
        !self.away && !self.roots.is_empty()
    }

    /// The open folders, to ask whether they changed.
    pub fn open_folders(&self) -> Vec<String> {
        self.open.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, folder: bool) -> Entry {
        Entry {
            name: name.into(),
            folder,
            size: 1,
            modified: 1,
        }
    }

    fn tree() -> Tree {
        let mut t = Tree {
            node: "n1".into(),
            ..Default::default()
        };
        t.rooted(vec![Root {
            name: "~".into(),
            path: "/home/p".into(),
        }]);
        t.listed("/home/p".into(), vec![e("b.md", false), e("src", true)]);
        t
    }

    fn names(t: &Tree) -> Vec<String> {
        t.rows()
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    #[test]
    fn a_first_tree_opens_its_first_root_and_lists_folders_first() {
        let t = tree();
        assert_eq!(names(&t), ["~", "  src", "  b.md"]);
    }

    #[test]
    fn opening_a_folder_reads_it_once_and_shows_it_inside() {
        let mut t = tree();
        assert!(t.toggle("/home/p/src"), "first opening reads it");
        assert_eq!(t.unread(), ["/home/p/src"]);
        t.listed("/home/p/src".into(), vec![e("main.rs", false)]);
        assert_eq!(names(&t), ["~", "  src", "    main.rs", "  b.md"]);
        assert!(!t.toggle("/home/p/src"), "closing reads nothing");
        assert!(!t.toggle("/home/p/src"), "reopening shows what was read");
    }

    #[test]
    fn keys_walk_the_rows_and_left_right_close_and_open() {
        let mut t = tree();
        t.step(1);
        assert_eq!(t.selected.as_deref(), Some("/home/p"));
        t.step(1);
        assert!(t.right(), "src opens and needs reading");
        t.listed("/home/p/src".into(), vec![e("main.rs", false)]);
        assert!(!t.right());
        assert_eq!(t.selected.as_deref(), Some("/home/p/src/main.rs"));
        t.left();
        assert_eq!(
            t.selected.as_deref(),
            Some("/home/p/src"),
            "a file picks its folder"
        );
        t.left();
        assert_eq!(names(&t), ["~", "  src", "  b.md"], "an open folder closes");
        t.step(isize::MAX);
        assert_eq!(t.selected.as_deref(), Some("/home/p/b.md"));
        assert_eq!(t.folder_here().as_deref(), Some("/home/p"));
    }

    #[test]
    fn reveal_opens_the_way_down_to_a_file() {
        let mut t = tree();
        assert_eq!(
            t.reveal("/home/p/src/world/gen.rs"),
            ["/home/p/src", "/home/p/src/world"]
        );
        assert!(t.open.contains("/home/p/src/world"));
        assert!(t.reveal("/etc/passwd").is_empty(), "outside the roots");
    }

    #[test]
    fn a_kept_tree_reopens_its_folders_under_the_roots_only() {
        let k = Kept {
            node: "n2".into(),
            open: vec!["/etc".into(), "/home/p/src".into()],
            selected: Some("/home/p/b.md".into()),
        };
        let mut t = Tree::from_kept(&k);
        assert_eq!(t.kept(), k);
        t.rooted(vec![Root {
            name: "~".into(),
            path: "/home/p".into(),
        }]);
        assert_eq!(t.open_folders(), ["/home/p", "/home/p/src"]);
        t.choose_node("n3");
        assert!(t.roots.is_empty() && t.open.is_empty());
    }
}
