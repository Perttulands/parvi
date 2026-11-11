//! The Brain page's state: the search as you type, the collections and
//! what arrived lately, the page in the reader with its history, the trail
//! that leads back, and an edit in hand (CHROTE `useBrainSearch`,
//! `PageReader`, `LibraryView`; Parvi `ui/src/brain/store.ts`). Plain data;
//! the page draws it and makes the requests.

use std::collections::BTreeSet;

/// How long typing must rest before a search goes out (CHROTE `SETTLE_MS`).
pub const SETTLE_MS: u64 = 200;
/// From this many results the list says it is the best of more.
pub const LIMIT: usize = 50;

pub use parvi_brain_contract::{Collection, Commit, Found as Hit, Link, Listed, Page, PageRef};

use parvi_brain_contract::{Changes, Collections, Listing, Results};

/// An edit in hand: the reader shows the editor instead of the page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edit {
    /// Identifies this draft, including if the same page is reopened.
    pub id: u64,
    /// Discard was pressed once; a second press throws the edit away.
    pub discarding: bool,
    pub saving: bool,
}

#[derive(Default)]
pub struct Brain {
    /// The query the results answer, and each search's number: an answer
    /// to an older one is dropped.
    pub query: String,
    pub asked: u32,
    pub searching: bool,
    pub results: Vec<Hit>,
    pub selected: usize,
    pub search_error: Option<String>,
    /// The way back: (path, title) of every page read since the trail
    /// began, the reader's last.
    pub trail: Vec<(String, String)>,
    pub page: Option<Page>,
    pub reading: u32,
    pub page_error: Option<String>,
    /// The page's history unfolded under its head.
    pub history_open: bool,
    pub edit: Option<Edit>,
    next_edit: u64,
    /// The collections, None until read.
    pub collections: Option<Vec<Collection>>,
    pub collections_error: Option<String>,
    /// The collection drawn open in the rail, and its pages: None while
    /// they are read, so an empty collection and an unread one differ.
    pub open_collection: Option<String>,
    pub listing: Option<Vec<Listed>>,
    /// Each page that arrived lately, once, newest first.
    pub arrivals: Vec<Listed>,
    /// What git said when it would not read the corpus.
    pub git_error: Option<String>,
    /// Collections left out of the search and the arrivals, kept per device.
    pub hidden: BTreeSet<String>,
}

impl Brain {
    /// The query changed: a number for the search to come, or None when
    /// the query is empty and the results simply clear.
    pub fn typed(&mut self, query: &str) -> Option<u32> {
        self.asked += 1;
        self.query = query.trim().to_string();
        self.search_error = None;
        if self.query.is_empty() {
            self.results.clear();
            self.selected = 0;
            self.searching = false;
            return None;
        }
        self.searching = true;
        Some(self.asked)
    }

    /// A search answered. False if it was not the latest.
    pub fn found(&mut self, asked: u32, status: u16, body: &[u8]) -> bool {
        if asked != self.asked {
            return false;
        }
        self.searching = false;
        match serde_json::from_slice::<Results>(body) {
            Ok(r) if status == 200 => {
                self.results = r.results;
                self.selected = 0;
            }
            _ => self.search_error = Some(words(status, body)),
        }
        true
    }

    /// The results shown: a hidden collection's are left out.
    pub fn shown(&self) -> Vec<&Hit> {
        self.results
            .iter()
            .filter(|h| !self.hidden.contains(&h.collection))
            .collect()
    }

    /// Up or Down: the result to open, clamped to the list.
    pub fn step(&mut self, by: isize) -> Option<String> {
        let shown = self.shown();
        if shown.is_empty() {
            return None;
        }
        let last = shown.len() as isize - 1;
        let at = (self.selected as isize + by).clamp(0, last) as usize;
        let path = shown[at].path.clone();
        self.selected = at;
        Some(path)
    }

    /// Hides a shown collection or shows a hidden one.
    pub fn toggle_hidden(&mut self, name: &str) {
        if !self.hidden.remove(name) {
            self.hidden.insert(name.to_string());
        }
        self.selected = 0;
    }

    /// The arrivals shown: a hidden collection's are left out.
    pub fn arrivals_shown(&self) -> Vec<&Listed> {
        self.arrivals
            .iter()
            .filter(|a| !self.hidden.contains(collection_of(&a.page.path)))
            .collect()
    }

    /// The collections answered.
    pub fn collections_read(&mut self, status: u16, body: &[u8]) {
        match serde_json::from_slice::<Collections>(body) {
            Ok(c) if status == 200 => {
                self.collections = Some(c.collections);
                self.collections_error = None;
            }
            _ => self.collections_error = Some(words(status, body)),
        }
    }

    /// The recent changes answered: each page once, at its newest change.
    pub fn changes_read(&mut self, status: u16, body: &[u8]) {
        match serde_json::from_slice::<Changes>(body) {
            Ok(c) if status == 200 => {
                let mut arrivals: Vec<Listed> = vec![];
                for change in c.changes {
                    for p in change.pages {
                        if arrivals.iter().all(|a| a.page.path != p.path) {
                            arrivals.push(Listed {
                                page: p,
                                updated: change.commit.time.clone(),
                                author: change.commit.author.clone(),
                            });
                        }
                    }
                }
                self.arrivals = arrivals;
                self.git_error = c.error;
            }
            _ => self.git_error = Some(words(status, body)),
        }
    }

    /// Opens a collection in the rail, or closes it if it is open: Some
    /// when its pages are to be read.
    pub fn toggle_collection(&mut self, name: &str) -> Option<String> {
        self.listing = None;
        if self.open_collection.as_deref() == Some(name) {
            self.open_collection = None;
            return None;
        }
        self.open_collection = Some(name.to_string());
        self.open_collection.clone()
    }

    /// A collection's pages answered; dropped if another is open now.
    pub fn listing_read(&mut self, name: &str, status: u16, body: &[u8]) {
        if self.open_collection.as_deref() != Some(name) {
            return;
        }
        match serde_json::from_slice::<Listing>(body) {
            Ok(l) if status == 200 => {
                self.listing = Some(l.pages);
                if l.error.is_some() {
                    self.git_error = l.error;
                }
            }
            _ => {
                self.listing = Some(vec![]);
                self.git_error = Some(words(status, body));
            }
        }
    }

    /// Edit: the page in hand goes into the editor. False with no page.
    pub fn start_edit(&mut self) -> bool {
        if self.page.is_none() {
            return false;
        }
        self.next_edit += 1;
        self.edit = Some(Edit {
            id: self.next_edit,
            ..Edit::default()
        });
        true
    }

    /// Discard: the first press asks, the second throws the edit away.
    /// True when the edit is gone.
    pub fn discard(&mut self) -> bool {
        match &mut self.edit {
            Some(e) if !e.discarding => {
                e.discarding = true;
                false
            }
            _ => {
                self.edit = None;
                true
            }
        }
    }

    /// A save answered: the page carries the new text and its commit on
    /// top of its history, and the editor closes. On a refusal, why, and
    /// the edit stays. If the draft changed while saving, keep that draft
    /// open against the text that actually reached the corpus.
    pub fn saved(
        &mut self,
        id: u64,
        path: &str,
        text: &str,
        keep_edit: bool,
        status: u16,
        body: &[u8],
    ) -> Result<Commit, String> {
        let current = self.edit.as_ref().is_some_and(|e| e.id == id);
        if current && let Some(e) = &mut self.edit {
            e.saving = false;
        }
        let commit = match serde_json::from_slice::<Commit>(body) {
            Ok(c) if status == 200 => c,
            _ => return Err(words(status, body)),
        };
        // The draft id decides only the edit; the page shows what the node
        // committed whenever it is the page saved, even if its draft was
        // discarded meanwhile, so a later edit starts from that commit.
        if current && !keep_edit {
            self.edit = None;
        }
        if let Some(p) = self.page.as_mut().filter(|p| p.path == path) {
            let split = text.len() - split_front_matter(text);
            p.front = text[..split].to_string();
            p.body = text[split..].to_string();
            if p.history.first().is_none_or(|c| c.hash != commit.hash) {
                p.history.insert(0, commit.clone());
            }
            p.updated = commit.time.clone();
            p.author = commit.author.clone();
        }
        Ok(commit)
    }

    /// Asking for a page: the number its answer must carry. It ends an
    /// edit and folds the history, as CHROTE's reader does.
    pub fn open(&mut self) -> u32 {
        self.edit = None;
        self.history_open = false;
        self.reading += 1;
        self.reading
    }

    /// A page answered. False if another page was asked for since. A page
    /// read joins the trail, or the trail goes back to it.
    pub fn read(&mut self, reading: u32, status: u16, body: &[u8]) -> bool {
        if reading != self.reading {
            return false;
        }
        match serde_json::from_slice::<Page>(body) {
            Ok(p) if status == 200 => {
                match self.trail.iter().position(|(path, _)| *path == p.path) {
                    Some(at) => self.trail.truncate(at + 1),
                    None => self.trail.push((p.path.clone(), p.title.clone())),
                }
                self.page = Some(p);
                self.page_error = None;
            }
            _ => self.page_error = Some(words(status, body)),
        }
        true
    }

    pub fn close(&mut self) {
        self.page = None;
        self.trail.clear();
        self.page_error = None;
        self.history_open = false;
        self.edit = None;
        self.reading += 1;
    }

    /// The count line under the search.
    pub fn count(&self) -> String {
        if self.searching {
            return "Searching…".into();
        }
        match self.shown().len() {
            0 => "Nothing found".into(),
            1 => "1 page".into(),
            n if n >= LIMIT => format!("{n} pages, best first"),
            n => format!("{n} pages"),
        }
    }

    /// What Alt+S pastes: CHROTE's reference to the page, or the Brain.
    pub fn reference(&self) -> String {
        match &self.page {
            Some(p) => format!("library {}", p.path),
            None => "library".into(),
        }
    }
}

/// The collection a page sits in: the first folder of its path.
pub fn collection_of(path: &str) -> &str {
    path.split_once('/').map_or("", |(top, _)| top)
}

/// How long the body after `text`'s front matter is, read as the node reads
/// it: a `---` line opens it and the next `---` line closes it.
fn split_front_matter(text: &str) -> usize {
    let Some(rest) = text.strip_prefix("---\n") else {
        return text.len();
    };
    let mut at = 4;
    for line in rest.split_inclusive('\n') {
        at += line.len();
        if line.trim_end() == "---" {
            return text.len() - at;
        }
    }
    text.len()
}

/// When `iso` was, from `now`, as the Library says it: `3 days ago`.
pub fn when(iso: &str, now: jiff::Timestamp) -> String {
    let Ok(at) = iso.parse::<jiff::Timestamp>() else {
        return "never".into();
    };
    let s = (now.as_second() - at.as_second()).max(0);
    let say = |n: i64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;
    match s {
        s if s < MINUTE => "just now".into(),
        s if s < HOUR => say(s / MINUTE, "minute"),
        s if s < DAY => say(s / HOUR, "hour"),
        s if s < MONTH => say(s / DAY, "day"),
        s if s < YEAR => say(s / MONTH, "month"),
        s => say(s / YEAR, "year"),
    }
}

/// A query parameter's value, percent-encoded.
pub fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Where a link leads, among the page's resolved links.
pub fn resolve<'a>(page: &'a Page, target: &str) -> Option<&'a PageRef> {
    page.links
        .iter()
        .find(|l| l.target == target)
        .and_then(|l| l.page.as_ref())
}

/// A snippet's parts: (text, marked) with `»…«` marking the match.
pub fn marks(snippet: &str) -> Vec<(String, bool)> {
    let mut out = vec![];
    let mut rest = snippet;
    while let Some(open) = rest.find('»') {
        let Some(len) = rest[open..].find('«') else {
            break;
        };
        if open > 0 {
            out.push((rest[..open].to_string(), false));
        }
        let inner = &rest[open + '»'.len_utf8()..open + len];
        out.push((inner.to_string(), true));
        rest = &rest[open + len + '«'.len_utf8()..];
    }
    if !rest.is_empty() {
        out.push((rest.to_string(), false));
    }
    out
}

/// Display a snippet without heading or wiki-link syntax, retaining match marks.
pub fn snippet_parts(snippet: &str) -> Vec<(String, bool)> {
    let chars: Vec<(char, bool)> = marks(snippet)
        .into_iter()
        .flat_map(|(text, marked)| text.chars().map(move |c| (c, marked)).collect::<Vec<_>>())
        .collect();
    let mut visible = Vec::new();
    let mut i = 0;
    let mut line_start = true;
    while i < chars.len() {
        if line_start {
            let mut end = i;
            while end < chars.len() && matches!(chars[end].0, ' ' | '\t') {
                end += 1;
            }
            let hashes = end;
            while end < chars.len() && chars[end].0 == '#' {
                end += 1;
            }
            if end > hashes && end < chars.len() && chars[end].0 == ' ' {
                i = end + 1;
                line_start = false;
                continue;
            }
        }
        if chars[i].0 == '['
            && chars.get(i + 1).is_some_and(|c| c.0 == '[')
            && let Some(end) = (i + 2..chars.len().saturating_sub(1))
                .find(|&j| chars[j].0 == ']' && chars[j + 1].0 == ']')
        {
            let start = (i + 2..end)
                .find(|&j| chars[j].0 == '|')
                .map_or(i + 2, |j| j + 1);
            visible.extend_from_slice(&chars[start..end]);
            i = end + 2;
            line_start = false;
            continue;
        }
        visible.push(chars[i]);
        line_start = chars[i].0 == '\n';
        i += 1;
    }
    let mut parts: Vec<(String, bool)> = Vec::new();
    for (ch, marked) in visible {
        if let Some((text, last)) = parts.last_mut()
            && *last == marked
        {
            text.push(ch);
        } else {
            parts.push((ch.to_string(), marked));
        }
    }
    parts
}

fn words(status: u16, body: &[u8]) -> String {
    if status == 0 {
        return String::from_utf8_lossy(body).into_owned();
    }
    parvi_app_kit::link::http::error_text(status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_query_clears_at_once_and_a_stale_answer_is_dropped() {
        let mut b = Brain::default();
        let first = b.typed("pa").unwrap();
        let second = b.typed("pace").unwrap();
        assert!(!b.found(first, 200, br#"{"results":[{"path":"a.md","title":"A"}]}"#));
        assert!(b.found(
            second,
            200,
            br#"{"results":[{"path":"pace.md","title":"Pace"}]}"#
        ));
        assert_eq!(b.results[0].path, "pace.md");
        assert_eq!(b.typed("  "), None);
        assert!(b.results.is_empty());
    }

    #[test]
    fn up_and_down_move_and_open_clamped_to_the_list() {
        let mut b = Brain::default();
        let n = b.typed("x").unwrap();
        b.found(
            n,
            200,
            br#"{"results":[{"path":"a","title":"A"},{"path":"b","title":"B"}]}"#,
        );
        assert_eq!(b.step(1).as_deref(), Some("b"));
        assert_eq!(b.step(1).as_deref(), Some("b"));
        assert_eq!(b.step(-5).as_deref(), Some("a"));
    }

    #[test]
    fn the_trail_goes_back_to_a_page_already_on_it() {
        let mut b = Brain::default();
        for p in ["a", "b", "c", "b"] {
            let r = b.open();
            b.read(
                r,
                200,
                format!(r#"{{"path":"{p}","title":"{p}"}}"#).as_bytes(),
            );
        }
        let paths: Vec<&str> = b.trail.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["a", "b"]);
        let r = b.open();
        assert!(!b.read(r - 1, 200, br#"{"path":"c","title":"C"}"#));
        assert!(b.read(
            r,
            404,
            br#"{"error":{"code":"not_found","message":"No such page"}}"#
        ));
        assert_eq!(b.page_error.as_deref(), Some("No such page"));
        assert_eq!(
            b.page.as_ref().unwrap().path,
            "b",
            "a failed read keeps the page"
        );
        assert_eq!(encode("self/pace & me"), "self/pace%20%26%20me");
    }

    #[test]
    fn closing_a_dive_folds_history_and_rejects_an_in_flight_page() {
        let mut b = Brain::default();
        let first = b.open();
        b.read(
            first,
            200,
            br#"{"path":"preferences/workflow.md","title":"Workflow"}"#,
        );
        assert_eq!(b.reference(), "library preferences/workflow.md");
        b.history_open = true;
        let pending = b.open();
        b.close();
        assert!(!b.read(
            pending,
            200,
            br#"{"path":"preferences/tools.md","title":"Tools"}"#
        ));
        assert!(b.page.is_none());
        assert!(b.trail.is_empty());
        assert!(!b.history_open);
        assert_eq!(b.reference(), "library");
    }

    #[test]
    fn counts_as_chrote_does() {
        let mut b = Brain::default();
        let n = b.typed("x").unwrap();
        assert_eq!(b.count(), "Searching…");
        let many: Vec<String> = (0..50)
            .map(|i| format!(r#"{{"path":"{i}","title":"{i}"}}"#))
            .collect();
        b.found(
            n,
            200,
            format!(r#"{{"results":[{}]}}"#, many.join(",")).as_bytes(),
        );
        assert_eq!(b.count(), "50 pages, best first");
    }

    #[test]
    fn a_hidden_collection_leaves_the_search_and_the_arrivals() {
        let mut b = Brain::default();
        let n = b.typed("x").unwrap();
        b.found(
            n,
            200,
            br#"{"results":[{"path":"self/a.md","title":"A","collection":"self"},
                {"path":"knowledge/b.md","title":"B","collection":"knowledge"}]}"#,
        );
        b.changes_read(
            200,
            br#"{"changes":[
                {"hash":"2","time":"2026-11-02T00:00:00Z","author":"L","message":"m",
                 "pages":[{"path":"self/a.md","title":"A"},{"path":"knowledge/b.md","title":"B"}]},
                {"hash":"1","time":"2026-11-01T00:00:00Z","author":"L","message":"m",
                 "pages":[{"path":"self/a.md","title":"A"}]}],"error":null}"#,
        );
        assert_eq!(b.arrivals.len(), 2, "each page once");
        assert_eq!(
            b.arrivals[0].updated, "2026-11-02T00:00:00Z",
            "at its newest"
        );
        b.toggle_hidden("self");
        assert_eq!(b.count(), "1 page");
        assert_eq!(b.step(1).as_deref(), Some("knowledge/b.md"));
        let arrived: Vec<&str> = b
            .arrivals_shown()
            .iter()
            .map(|a| a.page.path.as_str())
            .collect();
        assert_eq!(arrived, ["knowledge/b.md"]);
        b.toggle_hidden("self");
        assert_eq!(b.shown().len(), 2);
    }

    #[test]
    fn a_collection_opens_once_and_a_late_listing_is_dropped() {
        let mut b = Brain::default();
        assert_eq!(b.toggle_collection("self").as_deref(), Some("self"));
        assert_eq!(
            b.toggle_collection("knowledge").as_deref(),
            Some("knowledge")
        );
        b.listing_read(
            "self",
            200,
            br#"{"pages":[{"path":"self/a.md","title":"A"}]}"#,
        );
        assert_eq!(b.listing, None, "self is no longer open");
        b.listing_read(
            "knowledge",
            200,
            br#"{"pages":[{"path":"knowledge/b.md","title":"B","updated":"x","author":"L"}],"error":null}"#,
        );
        assert_eq!(b.listing.as_ref().unwrap()[0].page.title, "B");
        assert_eq!(b.toggle_collection("knowledge"), None);
        assert_eq!(b.open_collection, None);
    }

    #[test]
    fn an_edit_is_discarded_on_the_second_press_and_a_save_lands_on_top() {
        let mut b = Brain::default();
        assert!(!b.start_edit(), "no page, no edit");
        let r = b.open();
        b.read(
            r,
            200,
            br##"{"path":"self/a.md","title":"A","front":"---\nkind: note\n---\n","body":"# A\n",
                "history":[{"hash":"1","time":"t","author":"L","message":"First"}]}"##,
        );
        assert_eq!(
            b.page.as_ref().unwrap().text(),
            "---\nkind: note\n---\n# A\n"
        );
        assert!(b.start_edit());
        assert!(!b.discard(), "the first press asks");
        assert!(b.discard());
        assert_eq!(b.edit, None);

        b.start_edit();
        let id = b.edit.as_ref().unwrap().id;
        let refused = b.saved(
            id,
            "self/a.md",
            "x",
            false,
            409,
            br#"{"error":{"code":"conflict","message":"dirty"}}"#,
        );
        assert_eq!(refused, Err("dirty".into()));
        assert!(b.edit.is_some(), "a refused save keeps the edit");
        let text = "---\nkind: note\n---\n# A\n\nMore.\n";
        let commit = b
            .saved(
                id,
                "self/a.md",
                text,
                false,
                200,
                br#"{"hash":"2","time":"2026-11-09T10:00:00Z","author":"Ada","message":"More"}"#,
            )
            .unwrap();
        assert_eq!(commit.hash, "2");
        let p = b.page.as_ref().unwrap();
        assert_eq!(
            (p.front.as_str(), p.body.as_str()),
            ("---\nkind: note\n---\n", "# A\n\nMore.\n")
        );
        let hashes: Vec<&str> = p.history.iter().map(|c| c.hash.as_str()).collect();
        assert_eq!(hashes, ["2", "1"]);
        assert_eq!((p.author.as_str(), b.edit.is_none()), ("Ada", true));
    }

    #[test]
    fn a_late_save_does_not_change_another_page_or_a_new_draft() {
        let mut b = Brain::default();
        let r = b.open();
        b.read(r, 200, br##"{"path":"a.md","title":"A","body":"# A\n"}"##);
        b.start_edit();
        let old = b.edit.as_ref().unwrap().id;
        let r = b.open();
        b.read(r, 200, br##"{"path":"b.md","title":"B","body":"# B\n"}"##);
        b.start_edit();
        let current = b.edit.clone();
        let commit = br#"{"hash":"1","time":"t","author":"Ada","message":"Edit A"}"#;
        b.saved(old, "a.md", "# Changed A\n", false, 200, commit)
            .unwrap();
        assert_eq!(b.page.as_ref().unwrap().body, "# B\n");
        assert_eq!(b.edit, current);

        b.discard();
        b.discard();
        b.start_edit();
        b.edit.as_mut().unwrap().saving = true;
        let newer = b.edit.clone();
        b.saved(
            current.unwrap().id,
            "b.md",
            "# Old draft\n",
            false,
            200,
            commit,
        )
        .unwrap();
        assert_eq!(
            b.page.as_ref().unwrap().body,
            "# Old draft\n",
            "the page shows what the node committed"
        );
        assert_eq!(b.edit, newer, "the newer draft stays open");
        assert_eq!(
            b.saved(
                old,
                "a.md",
                "x",
                false,
                409,
                br#"{"error":{"code":"conflict","message":"dirty"}}"#
            ),
            Err("dirty".into())
        );
        assert_eq!(
            b.edit, newer,
            "a stale refusal does not release a newer save"
        );
    }

    #[test]
    fn a_save_lands_on_its_page_after_the_draft_was_discarded() {
        let mut b = Brain::default();
        let r = b.open();
        b.read(r, 200, br##"{"path":"a.md","title":"A","body":"# A\n"}"##);
        b.start_edit();
        let id = b.edit.as_ref().unwrap().id;
        b.edit.as_mut().unwrap().saving = true;
        b.discard();
        b.discard();
        assert!(b.edit.is_none());
        let commit = br#"{"hash":"1","time":"t","author":"Ada","message":"Edit A"}"#;
        b.saved(id, "a.md", "# Saved A\n", false, 200, commit)
            .unwrap();
        let page = b.page.as_ref().unwrap();
        assert_eq!(
            page.body, "# Saved A\n",
            "the next edit starts from the commit"
        );
        assert_eq!(page.history[0].hash, "1");
    }

    #[test]
    fn a_save_keeps_a_draft_changed_while_the_commit_was_pending() {
        let mut b = Brain::default();
        let r = b.open();
        b.read(r, 200, br##"{"path":"a.md","title":"A","body":"# A\n"}"##);
        b.start_edit();
        let id = b.edit.as_ref().unwrap().id;
        b.edit.as_mut().unwrap().saving = true;
        let commit = br#"{"hash":"1","time":"t","author":"Ada","message":"Edit A"}"#;
        b.saved(id, "a.md", "# Saved\n", true, 200, commit).unwrap();
        assert_eq!(b.page.as_ref().unwrap().body, "# Saved\n");
        assert_eq!(b.page.as_ref().unwrap().history[0].hash, "1");
        assert_eq!(b.edit.as_ref().unwrap().id, id);
        assert!(!b.edit.as_ref().unwrap().saving);
    }

    #[test]
    fn says_when_as_the_library_does() {
        let now: jiff::Timestamp = "2026-11-09T12:00:00Z".parse().unwrap();
        assert_eq!(when("2026-11-09T11:59:30Z", now), "just now");
        assert_eq!(when("2026-11-09T11:00:00+00:00", now), "1 hour ago");
        assert_eq!(when("2026-11-06T12:00:00+02:00", now), "3 days ago");
        assert_eq!(when("2025-10-01T00:00:00Z", now), "1 year ago");
        assert_eq!(when("", now), "never");
    }

    #[test]
    fn snippet_removes_markup_without_losing_match_marks() {
        assert_eq!(
            snippet_parts("## »Heading« [[Some Page|»shown«]] text"),
            vec![
                ("Heading".into(), true),
                (" ".into(), false),
                ("shown".into(), true),
                (" text".into(), false),
            ]
        );
        assert_eq!(
            snippet_parts("# One\n## [[Page]]"),
            vec![("One\nPage".into(), false)]
        );
        assert_eq!(
            snippet_parts("»[[Page|label]]«"),
            vec![("label".into(), true)]
        );
    }
    #[test]
    fn marks_the_matched_words_in_a_snippet() {
        assert_eq!(
            marks("the »pace« of work"),
            vec![
                ("the ".into(), false),
                ("pace".into(), true),
                (" of work".into(), false)
            ]
        );
    }
}
