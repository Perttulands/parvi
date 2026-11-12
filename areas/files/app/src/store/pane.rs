//! One Files pane: a device, a folder on it, what is selected, the find
//! beneath it, and the file open in it. No drawing and no fetching: the
//! page asks, then hands the answers here.

use super::listing::{self, Sort};
use super::paths;
use parvi_files_contract::{Entry, Matches, Root};
use serde::{Deserialize, Serialize};

/// Where a pane was, kept across restarts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    /// The node's id; empty means the serving node.
    pub node: String,
    pub path: Option<String>,
    pub selected: Option<String>,
    pub sort: Sort,
}

/// Find beneath the folder: by name, or by the text inside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Find {
    pub query: String,
    pub content: bool,
    /// None while the node is looking.
    pub found: Option<Result<Matches, String>>,
    pub cursor: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Pane {
    pub node: String,
    pub roots: Vec<Root>,
    pub path: Option<String>,
    pub entries: Vec<Entry>,
    pub sort: Sort,
    pub filter: String,
    pub selected: Option<String>,
    /// Why the folder (or the device's roots) could not be read.
    pub error: Option<String>,
    /// The device is away: the listing is the last one read, and nothing
    /// that writes is offered.
    pub away: bool,
    pub find: Option<Find>,
    pub find_revision: u64,
    /// Delete was pressed once on the selection; the next press deletes.
    pub armed: bool,
    /// A listing or roots request in flight, so answers to older ones are
    /// dropped.
    pub asked: u64,
}

impl Pane {
    pub fn from_kept(k: &Kept) -> Self {
        Self {
            node: k.node.clone(),
            path: k.path.clone(),
            selected: k.selected.clone(),
            sort: k.sort,
            ..Default::default()
        }
    }

    pub fn kept(&self) -> Kept {
        Kept {
            node: self.node.clone(),
            path: self.path.clone(),
            selected: self.selected.clone(),
            sort: self.sort,
        }
    }

    pub fn shown(&self) -> Vec<&Entry> {
        listing::shown(&self.entries, &self.filter, self.sort)
    }

    pub fn root(&self) -> Option<&Root> {
        self.path
            .as_deref()
            .and_then(|p| paths::root_of(&self.roots, p))
    }

    /// The selected entry while it is shown: one the filter hides is not
    /// offered to act on.
    pub fn selection(&self) -> Option<&Entry> {
        let s = self.selected.as_deref()?;
        self.shown().into_iter().find(|e| e.name == s)
    }

    /// The selected entry's full path.
    pub fn selected_path(&self) -> Option<String> {
        Some(paths::join(self.path.as_deref()?, &self.selection()?.name))
    }

    /// Delete pressed (by key, the toolbar or the menu alike): the first
    /// press on a selection arms it, the second gives what to delete. Only
    /// the operator's own presses count; nothing arms on their behalf.
    pub fn press_delete(&mut self) -> Option<String> {
        let path = self.selected_path().filter(|_| self.writable())?;
        self.armed = !self.armed;
        (!self.armed).then_some(path)
    }

    /// A new device: what was shown belongs to the old one.
    pub fn choose_node(&mut self, node: &str) {
        if self.node != node {
            self.node = node.to_string();
            self.roots.clear();
            self.path = None;
            self.entries.clear();
            self.selected = None;
            self.find = None;
            self.filter.clear();
            self.error = None;
        }
    }

    /// The folder to open once the roots are known: the one this pane last
    /// showed if it is under them, else its own root (the second pane takes
    /// the second root when there are several).
    pub fn start_path(&self) -> Option<String> {
        if let Some(p) = &self.path
            && paths::root_of(&self.roots, p).is_some()
        {
            return Some(p.clone());
        }
        self.roots.first().map(|r| r.path.clone())
    }

    /// A listing arrived. A new folder starts unfiltered and unselected; the
    /// same folder keeps its selection while the entry is still there.
    pub fn listed(&mut self, path: String, entries: Vec<Entry>, select: Option<String>) {
        let same = self.path.as_deref() == Some(path.as_str());
        self.path = Some(path);
        self.entries = entries;
        self.error = None;
        self.away = false;
        self.armed = false;
        if !same {
            self.filter.clear();
            self.find = None;
            self.selected = None;
        }
        if select.is_some() {
            self.selected = select;
        }
        if !self
            .entries
            .iter()
            .any(|e| Some(&e.name) == self.selected.as_ref())
        {
            self.selected = None;
        }
    }

    /// Moves the selection by `by` rows of what is shown; the first press
    /// with nothing selected takes the first (or last) row.
    pub fn step(&mut self, by: isize) -> Option<&Entry> {
        let names: Vec<String> = self.shown().iter().map(|e| e.name.clone()).collect();
        if names.is_empty() {
            return None;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|s| names.iter().position(|n| n == s));
        let next = match at {
            Some(i) => (i as isize)
                .saturating_add(by)
                .clamp(0, names.len() as isize - 1) as usize,
            None if by < 0 => names.len() - 1,
            None => 0,
        };
        self.selected = Some(names[next].clone());
        self.armed = false;
        self.selection()
    }

    pub fn select(&mut self, name: &str) {
        if self.selected.as_deref() != Some(name) {
            self.armed = false;
        }
        self.selected = Some(name.to_string());
    }

    /// The folder above, unless this is a root.
    pub fn up(&self) -> Option<String> {
        let p = self.path.as_deref()?;
        if self.roots.iter().any(|r| r.path == p) {
            return None;
        }
        let up = paths::parent(p);
        paths::root_of(&self.roots, &up).map(|_| up)
    }

    /// The selection after `up()`: the folder just left.
    pub fn came_from(&self) -> Option<String> {
        self.path
            .as_deref()
            .map(|p| paths::base_name(p).to_string())
    }

    /// Whether writes are offered: the device answers and a folder is shown.
    pub fn writable(&self) -> bool {
        !self.away && self.path.is_some()
    }

    pub fn set_find(&mut self, query: &str, content: bool) -> u64 {
        self.find_revision = self.find_revision.wrapping_add(1);
        if query.trim().is_empty() {
            self.find = None;
        } else {
            self.find = Some(Find {
                query: query.to_string(),
                content,
                found: None,
                cursor: 0,
            });
        }
        self.find_revision
    }

    /// Find results: dropped when they answer a query no longer asked.
    pub fn found(&mut self, revision: u64, found: Result<Matches, String>) {
        if let Some(f) = &mut self.find
            && self.find_revision == revision
        {
            f.found = Some(found);
            f.cursor = 0;
        }
    }

    pub fn find_step(&mut self, by: isize) {
        if let Some(f) = &mut self.find {
            let n = match &f.found {
                Some(Ok(m)) => m.matches.len(),
                _ => 0,
            };
            if n > 0 {
                f.cursor = (f.cursor as isize + by).clamp(0, n as isize - 1) as usize;
            }
        }
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

    fn pane() -> Pane {
        let mut p = Pane {
            node: "n1".into(),
            roots: vec![Root {
                name: "~".into(),
                path: "/home/p".into(),
            }],
            ..Default::default()
        };
        p.listed(
            "/home/p/src".into(),
            vec![
                e("b.md", false),
                e("lib", true),
                e("a10.md", false),
                e("a2.md", false),
            ],
            None,
        );
        p
    }

    #[test]
    fn keys_walk_what_is_shown() {
        let mut p = pane();
        assert_eq!(p.step(1).unwrap().name, "lib");
        assert_eq!(p.step(1).unwrap().name, "a2.md");
        assert_eq!(p.step(1).unwrap().name, "a10.md");
        assert_eq!(p.step(5).unwrap().name, "b.md");
        assert_eq!(p.step(isize::MIN).unwrap().name, "lib", "Home");
        assert_eq!(p.step(1).unwrap().name, "a2.md");
        assert_eq!(p.step(isize::MAX).unwrap().name, "b.md", "End");
        p.filter = "a".into();
        p.selected = None;
        assert_eq!(p.step(-1).unwrap().name, "a10.md");
    }

    #[test]
    fn up_stops_at_the_root_and_selects_where_it_came_from() {
        let mut p = pane();
        assert_eq!(p.up().as_deref(), Some("/home/p"));
        assert_eq!(p.came_from().as_deref(), Some("src"));
        p.listed("/home/p".into(), vec![e("src", true)], p.came_from());
        assert_eq!(p.selected.as_deref(), Some("src"));
        assert_eq!(p.up(), None);
    }

    #[test]
    fn a_relisting_keeps_the_selection_while_it_exists() {
        let mut p = pane();
        p.select("b.md");
        p.listed("/home/p/src".into(), vec![e("b.md", false)], None);
        assert_eq!(p.selected.as_deref(), Some("b.md"));
        p.listed("/home/p/src".into(), vec![e("c.md", false)], None);
        assert_eq!(p.selected, None);
        p.listed(
            "/home/p/src".into(),
            vec![e("c.md", false)],
            Some("c.md".into()),
        );
        assert_eq!(p.selected.as_deref(), Some("c.md"));
    }

    #[test]
    fn delete_takes_two_presses_on_the_same_row() {
        let mut p = pane();
        assert_eq!(p.press_delete(), None, "nothing selected");
        p.select("lib");
        assert_eq!(p.press_delete(), None, "the first press only arms");
        assert!(p.armed);
        p.select("b.md");
        assert!(!p.armed, "another row disarms");
        assert_eq!(p.press_delete(), None);
        assert_eq!(p.press_delete().as_deref(), Some("/home/p/src/b.md"));
        assert!(!p.armed);
        p.away = true;
        assert_eq!(p.press_delete(), None, "an away device deletes nothing");
        assert!(!p.armed);
    }

    #[test]
    fn a_selection_the_filter_hides_is_not_offered() {
        let mut p = pane();
        p.select("b.md");
        assert!(p.selection().is_some());
        p.filter = "lib".into();
        assert!(p.selection().is_none());
    }

    #[test]
    fn a_kept_pane_starts_where_it_was_or_at_its_root() {
        let k = Kept {
            node: "n2".into(),
            path: Some("/srv/x".into()),
            selected: Some("y".into()),
            sort: Sort::default(),
        };
        let mut p = Pane::from_kept(&k);
        assert_eq!(p.kept(), k);
        p.roots = vec![
            Root {
                name: "~".into(),
                path: "/home/p".into(),
            },
            Root {
                name: "/srv".into(),
                path: "/srv".into(),
            },
        ];
        assert_eq!(p.start_path().as_deref(), Some("/srv/x"));
        p.path = Some("/etc".into());
        assert_eq!(p.start_path().as_deref(), Some("/home/p"));
        p.choose_node("n3");
        assert!(p.path.is_none() && p.roots.is_empty());
    }

    #[test]
    fn stale_find_answers_are_dropped() {
        let mut p = pane();
        let stale = p.set_find("a", false);
        let current = p.set_find("ab", false);
        p.found(
            stale,
            Ok(Matches {
                matches: vec![],
                truncated: false,
            }),
        );
        assert!(p.find.as_ref().unwrap().found.is_none());
        p.found(
            current,
            Ok(Matches {
                matches: vec![],
                truncated: true,
            }),
        );
        assert!(p.find.as_ref().unwrap().found.is_some());
    }
}
