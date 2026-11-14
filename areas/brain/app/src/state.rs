//! The Brain page's state: the search as you type, what arrived lately,
//! the page in the reader with its history, the trail that leads back, and
//! an edit in hand. Plain data; the page draws it and makes the requests.

/// How long typing must rest before a search goes out (CHROTE `SETTLE_MS`).
pub const SETTLE_MS: u64 = 200;
/// From this many results the list says it is the best of more.
pub const LIMIT: usize = 50;

pub use parvi_brain_contract::{Commit, Found as Hit, Link, Listed, Page, PageRef};

use parvi_brain_contract::{Changes, Results};

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
    /// The way back: (path, title) of every page reached by a link since a
    /// page was opened from the list, the reader's last.
    pub trail: Vec<(String, String)>,
    pub page: Option<Page>,
    pub reading: u32,
    /// The page asked for was chosen from the list: it starts a new trail.
    fresh: bool,
    pub page_error: Option<String>,
    /// The page's history unfolded under its head.
    pub history_open: bool,
    pub edit: Option<Edit>,
    next_edit: u64,
    /// Each page that arrived lately, once, newest first, and whether the
    /// node has answered yet.
    pub arrivals: Vec<Listed>,
    pub arrivals_read: bool,
    /// What git said when it would not read the corpus.
    pub git_error: Option<String>,
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

    /// The pages the list shows: the results while searching, else what
    /// arrived lately.
    pub fn listed(&self) -> Vec<&str> {
        if self.query.is_empty() {
            self.arrivals.iter().map(|a| a.page.path.as_str()).collect()
        } else {
            self.results.iter().map(|h| h.path.as_str()).collect()
        }
    }

    /// The listed page the list marks: the chosen result, or the arrival
    /// in the reader.
    pub fn chosen(&self) -> Option<usize> {
        if self.query.is_empty() {
            let open = self.page.as_ref()?;
            self.listed().iter().position(|p| *p == open.path)
        } else {
            (self.selected < self.results.len()).then_some(self.selected)
        }
    }

    /// Up or Down: the listed page to open, clamped to the list; from the
    /// top when none is marked.
    pub fn step(&mut self, by: isize) -> Option<String> {
        let listed = self.listed();
        let last = listed.len().checked_sub(1)? as isize;
        let at = match self.chosen() {
            Some(from) => (from as isize + by).clamp(0, last) as usize,
            None => 0,
        };
        let path = listed[at].to_string();
        self.selected = at;
        Some(path)
    }

    /// The recent changes answered: each page once, at its newest change.
    pub fn changes_read(&mut self, status: u16, body: &[u8]) {
        self.arrivals_read = true;
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

    /// Asking for a page, `fresh` from the list or else by a link: the
    /// number its answer must carry. It ends an edit and folds the history,
    /// as CHROTE's reader does.
    pub fn open(&mut self, fresh: bool) -> u32 {
        self.fresh = fresh;
        self.edit = None;
        self.history_open = false;
        self.reading += 1;
        self.reading
    }

    /// A page answered. False if another page was asked for since. A page
    /// from the list starts the trail; one reached by a link joins it, or
    /// the trail goes back to it.
    pub fn read(&mut self, reading: u32, status: u16, body: &[u8]) -> bool {
        if reading != self.reading {
            return false;
        }
        match serde_json::from_slice::<Page>(body) {
            Ok(p) if status == 200 => {
                if self.fresh {
                    self.trail.clear();
                }
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

    /// How many pages the search found, beside it. With none it says
    /// nothing: the list says so, once.
    pub fn count(&self) -> Option<String> {
        if self.searching {
            return Some("Searching…".into());
        }
        match self.results.len() {
            0 => None,
            1 => Some("1 page".into()),
            n if n >= LIMIT => Some(format!("{n} pages, best first")),
            n => Some(format!("{n} pages")),
        }
    }

    /// What Alt+S pastes: the page's file, which an agent can open, or
    /// the Brain.
    pub fn reference(&self) -> String {
        match &self.page {
            Some(p) if !p.file.is_empty() => format!("brain page {}", p.file),
            Some(p) => format!("brain page {}", p.path),
            None => "the brain".into(),
        }
    }
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

/// Display a snippet without Markdown syntax (headings, wiki and Markdown
/// links, emphasis, code marks), retaining match marks.
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
        // A heading's marks the snippet ran onto one line: `text ## Heading`.
        if chars[i].0 == '#' && i > 0 && chars[i - 1].0 == ' ' {
            let end = (i..chars.len().min(i + 6))
                .take_while(|&j| chars[j].0 == '#')
                .last()
                .map_or(i, |j| j + 1);
            if chars.get(end).is_some_and(|c| c.0 == ' ') {
                i = end + 1;
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
        // A link the snippet cut through shows its words: `[[` without its
        // end, an end without its start.
        if matches!(
            (chars[i].0, chars.get(i + 1).map(|c| c.0)),
            ('[', Some('[')) | (']', Some(']'))
        ) {
            i += 2;
            continue;
        }
        // A Markdown link shows its text, its target left out.
        if chars[i].0 == '['
            && let Some(close) = (i + 1..chars.len())
                .take_while(|&j| chars[j].0 != '\n' && chars[j].0 != '[')
                .find(|&j| chars[j].0 == ']')
            && chars.get(close + 1).is_some_and(|c| c.0 == '(')
        {
            visible.extend_from_slice(&chars[i + 1..close]);
            i = (close + 2..chars.len())
                .find(|&j| chars[j].0 == ')' || chars[j].0 == '\n')
                .map_or(chars.len(), |j| j + usize::from(chars[j].0 == ')'));
            line_start = false;
            continue;
        }
        // Emphasis and code marks.
        if chars[i].0 == '`' || (chars[i].0 == '*' && chars.get(i + 1).is_some_and(|c| c.0 == '*'))
        {
            i += if chars[i].0 == '`' { 1 } else { 2 };
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
    fn with_nothing_typed_the_keys_walk_the_arrivals_from_the_page_read() {
        let mut b = Brain::default();
        b.changes_read(
            200,
            br#"{"changes":[{"hash":"1","time":"t","author":"L","message":"m",
                "pages":[{"path":"a","title":"A"},{"path":"b","title":"B"},{"path":"c","title":"C"}]}],"error":null}"#,
        );
        assert_eq!(b.chosen(), None);
        assert_eq!(
            b.step(1).as_deref(),
            Some("a"),
            "from the top when none is read"
        );
        let r = b.open(true);
        b.read(r, 200, br#"{"path":"b","title":"B"}"#);
        assert_eq!(b.chosen(), Some(1));
        assert_eq!(b.step(1).as_deref(), Some("c"));
    }

    #[test]
    fn the_trail_goes_back_to_a_page_already_on_it_and_the_list_starts_a_new_one() {
        let mut b = Brain::default();
        for (p, fresh) in [
            ("z", true),
            ("a", true),
            ("b", false),
            ("c", false),
            ("b", false),
        ] {
            let r = b.open(fresh);
            b.read(
                r,
                200,
                format!(r#"{{"path":"{p}","title":"{p}"}}"#).as_bytes(),
            );
        }
        let paths: Vec<&str> = b.trail.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["a", "b"]);
        let r = b.open(false);
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
        let r = b.open(true);
        b.read(r, 200, br#"{"path":"d","title":"D"}"#);
        assert_eq!(b.trail, vec![("d".to_string(), "D".to_string())]);
    }

    #[test]
    fn closing_a_dive_folds_history_and_rejects_an_in_flight_page() {
        let mut b = Brain::default();
        let first = b.open(true);
        b.read(
            first,
            200,
            br#"{"path":"preferences/workflow.md","title":"Workflow"}"#,
        );
        assert_eq!(b.reference(), "brain page preferences/workflow.md");
        b.history_open = true;
        let pending = b.open(true);
        b.close();
        assert!(!b.read(
            pending,
            200,
            br#"{"path":"preferences/tools.md","title":"Tools"}"#
        ));
        assert!(b.page.is_none());
        assert!(b.trail.is_empty());
        assert!(!b.history_open);
        assert_eq!(b.reference(), "the brain");
        let page = b.open(true);
        b.read(
            page,
            200,
            br#"{"path":"a/b.md","file":"/srv/data/context/a/b.md","title":"B"}"#,
        );
        assert_eq!(b.reference(), "brain page /srv/data/context/a/b.md");
    }

    #[test]
    fn counts_as_chrote_does() {
        let mut b = Brain::default();
        let n = b.typed("x").unwrap();
        assert_eq!(b.count().as_deref(), Some("Searching…"));
        let many: Vec<String> = (0..50)
            .map(|i| format!(r#"{{"path":"{i}","title":"{i}"}}"#))
            .collect();
        b.found(
            n,
            200,
            format!(r#"{{"results":[{}]}}"#, many.join(",")).as_bytes(),
        );
        assert_eq!(b.count().as_deref(), Some("50 pages, best first"));
    }

    #[test]
    fn each_arrival_shows_once_at_its_newest_change() {
        let mut b = Brain::default();
        assert!(!b.arrivals_read);
        b.changes_read(
            200,
            br#"{"changes":[
                {"hash":"2","time":"2026-11-02T00:00:00Z","author":"L","message":"m",
                 "pages":[{"path":"self/a.md","title":"A"},{"path":"knowledge/b.md","title":"B"}]},
                {"hash":"1","time":"2026-11-01T00:00:00Z","author":"L","message":"m",
                 "pages":[{"path":"self/a.md","title":"A"}]}],"error":null}"#,
        );
        assert!(b.arrivals_read);
        assert_eq!(b.arrivals.len(), 2, "each page once");
        assert_eq!(
            b.arrivals[0].updated, "2026-11-02T00:00:00Z",
            "at its newest"
        );
    }

    #[test]
    fn an_edit_is_discarded_on_the_second_press_and_a_save_lands_on_top() {
        let mut b = Brain::default();
        assert!(!b.start_edit(), "no page, no edit");
        let r = b.open(true);
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
        let r = b.open(true);
        b.read(r, 200, br##"{"path":"a.md","title":"A","body":"# A\n"}"##);
        b.start_edit();
        let old = b.edit.as_ref().unwrap().id;
        let r = b.open(true);
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
        let r = b.open(true);
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
        let r = b.open(true);
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
        let plain = |s: &str| -> String { snippet_parts(s).into_iter().map(|(t, _)| t).collect() };
        assert_eq!(plain("and [[resource …"), "and resource …");
        assert_eq!(
            plain("pacing ## Current understanding, C# and #1"),
            "pacing Current understanding, C# and #1"
        );
        assert_eq!(plain("page]] and more"), "page and more");
        assert_eq!(
            plain("its [installer](/srv/tools/heavy/install.sh) for"),
            "its installer for"
        );
        assert_eq!(plain("see [the doc](/srv/cut…"), "see the doc");
        assert_eq!(
            plain("**bold** and `code` [x] stays"),
            "bold and code [x] stays"
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
