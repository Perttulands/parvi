//! The Work page (DESIGN.md, Work): one project's open work as a tree with
//! the item beside it, the whole store a search away, an item handed to a
//! session in one action; the same item beside a terminal that named it,
//! and the project's tree in a dock beside any page.
mod beside;
mod dock;
mod natural;
mod report;
pub mod store;
#[cfg(test)]
mod tests;
mod tokens;
mod view;

use gpui::{App, AppContext, Context, Entity, FocusHandle, KeyDownEvent, WeakEntity, Window};
use parvi_app_kit::ask::{Answers, Ask};
use parvi_app_kit::widgets::field::{self, FieldEvent, TextField};
use parvi_app_kit::widgets::select::Selectable;
use parvi_app_kit::{Host, Mounted, Page, mount};
use parvi_core::NodeId;
use parvi_terminals_contract::SessionList;
use parvi_work_contract::{SNAPSHOT, Snapshot, WATCH};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use store::work::outline::{self, Found, Line};
use store::work::{Board, RowKey, prose};

pub const NAME: &str = "work";
pub fn build(host: Host, window: &mut Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, Work::new)
}

/// What the list shows, in order: the rows the keys walk, and the labels
/// between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A row of the board, this deep in the tree; `away` when it is
    /// another project's.
    Item {
        row: usize,
        depth: usize,
        away: bool,
    },
    Label(&'static str),
    /// Matches left out of a long search.
    More(usize),
}

pub struct Work {
    host: Host,
    me: WeakEntity<Work>,
    focus: FocusHandle,
    /// The search, or the project's name while a project is being chosen.
    search: Entity<TextField>,
    report: Option<Entity<report::Report>>,
    snapshots: BTreeMap<String, Arc<Snapshot>>,
    /// A node's snapshot that arrived while another was being read.
    pending: BTreeMap<String, Vec<u8>>,
    reading: bool,
    /// Nodes whose link closed while their snapshot was being read: the
    /// read is dropped when it lands.
    gone: std::collections::HashSet<String>,
    /// Nodes still reading for a Refresh press, with the count of
    /// refreshes they had finished when it was pressed.
    refreshing: BTreeMap<String, u64>,
    board: Rc<Board>,
    /// The project chosen last, by its folder or repository; kept.
    project: Option<String>,
    query: String,
    /// Choosing a project: the search names one, the list holds them.
    picking: bool,
    picked: usize,
    entries: Vec<Entry>,
    selected: Option<RowKey>,
    selected_node: Option<String>,
    /// The page shows on behalf of a page beside which it lies.
    beside_from: Option<&'static str>,
    card_line: Option<(String, String)>,
    trail: Vec<(Option<String>, RowKey)>,
    prose: Rc<RefCell<Selectable>>,
    wanted: Option<(String, Option<String>)>,
    sessions: Option<Entity<SessionList>>,
    /// The list's scroll, and whether the row the keys reached is to be
    /// brought into view at the next draw.
    list_scroll: gpui::ScrollHandle,
    reveal: bool,
    /// The item's finished family shows, not one line saying how many.
    show_finished: bool,
    /// The item the dock shows instead of its tree.
    dock_item: Option<RowKey>,
}

impl Page for Work {
    const ID: &'static str = NAME;
    const TITLE: &'static str = "Work";
    const KEY: Option<char> = Some('O');
    const AREA: Option<&'static str> = Some(NAME);
    const KINDS: parvi_core::frame::Kinds = &[SNAPSHOT..=SNAPSHOT];
    const CHANNELS: &'static [u8] = &[WATCH];
    const ANSWERS: &'static [Answers] = &[Answers::Reveal("item")];
    fn frame(&mut self, node: &NodeId, _: u8, body: &[u8], _: &mut Window, cx: &mut Context<Self>) {
        self.gone.remove(node.as_str());
        self.pending.insert(node.as_str().into(), body.to_vec());
        self.read_pending(cx);
    }
    fn link(
        &mut self,
        node: &NodeId,
        state: Option<parvi_app_kit::link::client::LinkState>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if state.is_none() {
            self.snapshots.remove(node.as_str());
            self.pending.remove(node.as_str());
            self.refreshing.remove(node.as_str());
            self.gone.insert(node.as_str().into());
            self.rebuild(cx);
        }
    }
    fn answer(
        &mut self,
        ask: Ask,
        from: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Ask::Reveal {
            id,
            node,
            beside: true,
            ..
        } = ask
        {
            self.open_beside(from, id, node, window, cx);
        } else if let Ask::Reveal { id, node, .. } = ask {
            self.wanted = Some((id, node.map(|n| n.to_string())));
            self.open_wanted(cx);
        }
    }
    fn chords(&self, _: &App) -> Vec<parvi_app_kit::chords::Chord> {
        let me = self.me.clone();
        let close = self.me.clone();
        let report = self.me.clone();
        vec![
            parvi_app_kit::chords::Chord::new("r", "Report", move |window, cx| {
                let _ = report.update(cx, |work, cx| work.open_report(window, cx));
            })
            .alt("r", Some(false))
            .everywhere(),
            parvi_app_kit::chords::Chord::new("i", "Close the item", move |_, cx| {
                let _ = close.update(cx, |work, cx| {
                    if let Some(from) = work.beside_from.take() {
                        work.host.close_beside(from, cx);
                    }
                });
            })
            .alt("i", Some(false))
            .everywhere(),
            parvi_app_kit::chords::Chord::new("b", "Work beside", move |_, cx| {
                let _ = me.update(cx, |work, cx| work.toggle_dock(cx));
            })
            .alt("b", Some(false))
            .everywhere(),
        ]
    }
    fn focus(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }
}

impl Work {
    pub fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextField::new(cx, SEARCH));
        cx.subscribe_in(&search, window, |w: &mut Self, _, ev, window, cx| {
            w.search_event(*ev, window, cx)
        })
        .detach();
        let sessions = host.model::<SessionList>(cx);
        if let Some(s) = &sessions {
            cx.observe(s, |_, _, cx| cx.notify()).detach();
        }
        let settings: serde_json::Value = host.setting(cx).unwrap_or_default();
        Self {
            host,
            me: cx.weak_entity(),
            focus: cx.focus_handle(),
            search,
            report: None,
            snapshots: BTreeMap::new(),
            pending: BTreeMap::new(),
            reading: false,
            gone: Default::default(),
            refreshing: BTreeMap::new(),
            board: Rc::default(),
            project: settings["project"].as_str().map(str::to_owned),
            query: String::new(),
            picking: false,
            picked: 0,
            entries: Vec::new(),
            selected: None,
            selected_node: None,
            beside_from: None,
            card_line: None,
            trail: Vec::new(),
            prose: Rc::default(),
            wanted: None,
            sessions,
            dock_item: None,
            show_finished: false,
            list_scroll: gpui::ScrollHandle::new(),
            reveal: false,
        }
    }

    /// Reads the newest snapshot waiting, off the window's thread: a node
    /// with thousands of items sends megabytes, and a snapshot that comes
    /// while one is read replaces any older one still waiting.
    fn read_pending(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            return;
        }
        let Some((node, body)) = self.pending.pop_first() else {
            return;
        };
        self.reading = true;
        let others: Vec<(String, Arc<Snapshot>)> = self
            .snapshots
            .iter()
            .filter(|(n, _)| **n != node)
            .map(|(n, s)| (n.clone(), s.clone()))
            .collect();
        let read = cx.background_spawn(async move {
            let snapshot = Arc::new(serde_json::from_slice::<Snapshot>(&body).ok()?);
            let mut all: BTreeMap<String, Arc<Snapshot>> = others.into_iter().collect();
            all.insert(node.clone(), snapshot.clone());
            Some((node, snapshot, Board::from_snapshot(&combined(&all))))
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |w, cx| {
                w.reading = false;
                if let Some((node, snapshot, board)) = read {
                    // A node gone meanwhile stays gone.
                    if !w.gone.contains(&node) {
                        if w.refreshing
                            .get(&node)
                            .is_some_and(|&at| snapshot.refreshed > at)
                        {
                            w.refreshing.remove(&node);
                        }
                        w.snapshots.insert(node, snapshot);
                        w.set_board(board, cx);
                    }
                }
                w.read_pending(cx);
            });
        })
        .detach();
    }

    /// The board again from the snapshots held, at once.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let board = Board::from_snapshot(&combined(&self.snapshots));
        self.set_board(board, cx);
    }

    fn set_board(&mut self, board: Board, cx: &mut Context<Self>) {
        self.board = Rc::new(board);
        self.relist(cx);
        self.open_wanted(cx);
        cx.notify();
    }

    /// Project indices for a node in the combined board, which is built in
    /// the same sorted node order as the snapshot map.
    fn projects_on(&self, node: &str) -> Option<std::ops::Range<usize>> {
        let mut first = 0;
        for (id, snapshot) in &self.snapshots {
            let end = first + snapshot.projects.len();
            if id == node {
                return Some(first..end);
            }
            first = end;
        }
        None
    }
    fn node_for_project(&self, project: usize) -> Option<String> {
        self.snapshots
            .keys()
            .find(|node| self.projects_on(node).is_some_and(|r| r.contains(&project)))
            .cloned()
    }
    fn find_on(&self, reference: &str, node: Option<&str>) -> Option<usize> {
        if let Some(node) = node {
            let projects = self.projects_on(node)?;
            if let Some((index, _)) = self
                .board
                .rows
                .iter()
                .enumerate()
                .find(|(_, r)| projects.contains(&r.project) && r.reference == reference)
            {
                return Some(index);
            }
            let mut matches = self
                .board
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| projects.contains(&r.project) && r.id == reference);
            let first = matches.next()?.0;
            return matches.next().is_none().then_some(first);
        }
        self.board.find(reference)
    }
    fn open_wanted(&mut self, cx: &mut Context<Self>) {
        let found = self
            .wanted
            .as_ref()
            .and_then(|(r, n)| self.find_on(r, n.as_deref()));
        if let Some(row) = found {
            self.wanted = None;
            self.open(row, cx);
        }
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        for node in self.host.nodes(cx) {
            let done = self
                .snapshots
                .get(node.id.as_str())
                .map_or(0, |s| s.refreshed);
            self.refreshing.insert(node.id.as_str().into(), done);
            self.host.send(&node.id, vec![WATCH, 2], cx);
        }
        cx.notify();
    }

    /// The project of the session in the focused terminal, by its folder.
    fn session_project(&self, cx: &App) -> Option<usize> {
        let list = self.sessions.as_ref()?.read(cx);
        let focused = list.focused.as_ref()?;
        let session = list.sessions.iter().find(|s| {
            s.node == focused.node
                && s.name == focused.name
                && s.user.as_deref().unwrap_or("") == focused.user
        })?;
        outline::project_of_folder(&self.board, session.cwd.as_deref()?)
    }

    /// The project the page shows: the one chosen last, else the focused
    /// session's, else the one whose work moved last.
    pub fn project(&self, cx: &App) -> Option<usize> {
        self.project
            .as_ref()
            .and_then(|p| self.board.projects.iter().position(|x| &x.path == p))
            .or_else(|| self.session_project(cx))
            .or_else(|| {
                (0..self.board.rows.len())
                    .filter(|&i| outline::open(&self.board, i))
                    .max_by_key(|&i| self.board.rows[i].updated)
                    .map(|i| self.board.rows[i].project)
            })
    }

    /// The project beside a terminal: the focused session's, else the
    /// page's.
    fn dock_project(&self, cx: &App) -> Option<usize> {
        self.session_project(cx).or_else(|| self.project(cx))
    }

    /// Chooses a project, and keeps the choice.
    pub fn choose_project(&mut self, project: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.board.projects.get(project).map(|p| p.path.clone()) else {
            return;
        };
        let mut settings: serde_json::Value = self.host.setting(cx).unwrap_or_default();
        if !settings.is_object() {
            settings = serde_json::json!({});
        }
        settings["project"] = serde_json::json!(path);
        if let Err(e) = self.host.set_setting(&settings, cx) {
            self.host.toast(&e.message, cx);
        }
        self.project = Some(path);
        self.stop_picking(cx);
        self.selected = None;
        self.selected_node = None;
        self.trail.clear();
        self.relist(cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn start_picking(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picking = true;
        self.picked = self
            .project(cx)
            .and_then(|p| self.projects_listed(cx).iter().position(|&x| x == p))
            .unwrap_or(0);
        self.search.update(cx, |f, cx| {
            f.set_placeholder(PICK);
            f.set("", false, cx);
        });
        field::focus(&self.search, window, cx);
        self.reveal = true;
        cx.notify();
    }

    fn stop_picking(&mut self, cx: &mut Context<Self>) {
        if !self.picking {
            return;
        }
        self.picking = false;
        let query = self.query.clone();
        self.search.update(cx, |f, cx| {
            f.set_placeholder(SEARCH);
            f.set(&query, false, cx);
        });
    }

    /// The projects to choose from, most open work first, those the typed
    /// name matches.
    pub fn projects_listed(&self, cx: &App) -> Vec<usize> {
        let needle = self.search.read(cx).text.trim().to_lowercase();
        let open = outline::open_counts(&self.board);
        let mut order: Vec<usize> = (0..self.board.projects.len())
            .filter(|&p| {
                let info = &self.board.projects[p];
                needle.is_empty()
                    || info.label.to_lowercase().contains(&needle)
                    || info.path.to_lowercase().contains(&needle)
            })
            .collect();
        order.sort_by(|&a, &b| {
            open[b].cmp(&open[a]).then_with(|| {
                self.board.projects[a]
                    .label
                    .cmp(&self.board.projects[b].label)
            })
        });
        order
    }

    /// What the list holds now: the project's tree, or a search's matches.
    fn relist(&mut self, cx: &App) {
        let project = self.project(cx);
        self.entries = if self.query.trim().is_empty() {
            project.map_or_else(Vec::new, |p| {
                outline::tree(&self.board, p)
                    .into_iter()
                    .map(|Line { row, depth }| Entry::Item {
                        row,
                        depth,
                        away: self.board.rows[row].project != p,
                    })
                    .collect()
            })
        } else {
            let Found {
                here,
                elsewhere,
                more,
            } = outline::search(&self.board, project, &self.query);
            let mut entries: Vec<Entry> = here
                .into_iter()
                .map(|row| Entry::Item {
                    row,
                    depth: 0,
                    away: false,
                })
                .collect();
            if !elsewhere.is_empty() {
                entries.push(Entry::Label("Other projects"));
                entries.extend(elsewhere.into_iter().map(|row| Entry::Item {
                    row,
                    depth: 0,
                    away: true,
                }));
            }
            if more > 0 {
                entries.push(Entry::More(more));
            }
            entries
        };
    }

    /// The rows the list shows, in order.
    pub fn listed(&self) -> Vec<usize> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item { row, .. } => Some(*row),
                _ => None,
            })
            .collect()
    }

    /// The selected row's index in the board, if it is still there.
    pub fn selected_row(&self) -> Option<usize> {
        let key = self.selected.as_ref()?;
        let node = self.selected_node.as_deref();
        let range = match node {
            Some(n) => Some(self.projects_on(n)?),
            None => None,
        };
        self.board.rows.iter().position(|row| {
            range.as_ref().is_none_or(|r| r.contains(&row.project))
                && row.reference == key.reference
                && row.pull.is_some() == key.pull
        })
    }

    /// The row the item view shows: the one chosen, else the list's first.
    pub fn shown_row(&self) -> Option<usize> {
        self.selected_row()
            .or_else(|| self.listed().first().copied())
    }

    pub fn select(&mut self, row: usize, cx: &mut Context<Self>) {
        self.trail.clear();
        self.select_keeping_trail(row, cx);
    }

    fn select_keeping_trail(&mut self, row: usize, cx: &mut Context<Self>) {
        let selected = self.board.rows.get(row).map(RowKey::of);
        let selected_node = self
            .board
            .rows
            .get(row)
            .and_then(|r| self.node_for_project(r.project));
        if selected != self.selected || selected_node != self.selected_node {
            self.prose.borrow_mut().selection.clear();
            self.show_finished = false;
        }
        self.selected = selected;
        self.selected_node = selected_node;
        cx.notify();
    }

    /// Shows a row reached from anywhere: an item of another project
    /// shows without changing the project.
    pub fn open(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.board.rows.get(row).is_none() {
            return;
        }
        self.select(row, cx);
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let listed = self.listed();
        if listed.is_empty() {
            return;
        }
        let n = listed.len() as isize;
        // The row shown is where the walk starts, chosen or not.
        let at = match self
            .shown_row()
            .and_then(|r| listed.iter().position(|&x| x == r))
        {
            Some(c) => (c as isize + by).clamp(0, n - 1),
            None => 0,
        };
        self.select(listed[at as usize], cx);
        self.reveal = true;
    }

    fn search_event(&mut self, ev: FieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking {
            let n = self.projects_listed(cx).len();
            match ev {
                FieldEvent::Changed => self.picked = 0,
                FieldEvent::Up => {
                    self.picked = self.picked.saturating_sub(1);
                    self.reveal = true;
                }
                FieldEvent::Down => {
                    self.picked = (self.picked + 1).min(n.saturating_sub(1));
                    self.reveal = true;
                }
                FieldEvent::Submit => {
                    if let Some(&p) = self.projects_listed(cx).get(self.picked) {
                        self.choose_project(p, window, cx);
                    }
                }
                _ => {}
            }
            cx.notify();
            return;
        }
        match ev {
            FieldEvent::Changed => {
                self.query = self.search.read(cx).text.clone();
                self.relist(cx);
                if let Some(&first) = self.listed().first() {
                    self.select(first, cx);
                }
            }
            FieldEvent::Down | FieldEvent::Submit => {
                if let Some(&first) = self.listed().first()
                    && self.selected_row().is_none()
                {
                    self.select(first, cx);
                }
                window.focus(&self.focus, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.query.clear();
        self.search.update(cx, |f, cx| f.set("", false, cx));
        self.relist(cx);
    }

    /// Esc in the page's search: the project choice ends, else the search
    /// clears, else the keys go back to the list. Whether it was used.
    pub fn escape_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.search.read(cx).is_focused(window) {
            return false;
        }
        if self.picking {
            self.stop_picking(cx);
        } else if !self.query.is_empty() {
            self.clear_search(cx);
            return true;
        }
        window.focus(&self.focus, cx);
        cx.notify();
        true
    }

    /// The list's keys: ↑↓ or j/k walk it, `/` searches, `p` chooses the
    /// project, Esc clears the search; Alt+S hands over and Alt+Left goes
    /// back along the trail.
    pub fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = k.modifiers;
        if self.alt_key(e, window, cx) {
            return;
        }
        if m.control && !m.alt && k.key == "c" && self.copy_prose(cx) {
            cx.stop_propagation();
            return;
        }
        if m.alt || m.control || m.platform {
            return;
        }
        let handled = match k.key.as_str() {
            "up" | "k" => {
                self.step(-1, cx);
                true
            }
            "down" | "j" => {
                self.step(1, cx);
                true
            }
            "escape" if !self.query.is_empty() => {
                self.clear_search(cx);
                true
            }
            "/" => {
                field::focus(&self.search, window, cx);
                true
            }
            "p" => {
                self.start_picking(window, cx);
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// The page's Alt keys, wherever focus is on it: Alt+S sends, Alt+Left
    /// goes back. Whether the key was one.
    pub fn alt_key(
        &mut self,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &e.keystroke;
        let m = k.modifiers;
        if !m.alt || m.control || m.platform || m.shift {
            return false;
        }
        match k.key.as_str() {
            "s" => self.send(window, cx),
            "left" => self.back(cx),
            _ => return false,
        }
        cx.stop_propagation();
        true
    }

    /// Shows a row reached from the item in hand, which joins the trail.
    pub fn follow_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let from = self
            .shown_row()
            .map(|r| (self.selected_node.clone(), RowKey::of(&self.board.rows[r])));
        let mut trail = std::mem::take(&mut self.trail);
        self.select_keeping_trail(row, cx);
        if let Some(from) = from
            && (self.selected.as_ref() != Some(&from.1) || self.selected_node != from.0)
        {
            trail.push(from);
        }
        self.trail = trail;
    }

    /// Back along the trail to the item the last link was followed from;
    /// nothing with nothing behind. An item gone from the board is passed.
    pub fn back(&mut self, cx: &mut Context<Self>) {
        while let Some((node, key)) = self.trail.pop() {
            if let Some(row) = self.find_on(&key.reference, node.as_deref()) {
                self.select_keeping_trail(row, cx);
                cx.notify();
                return;
            }
        }
    }

    pub(crate) fn item_menu(&self, row: usize) -> Vec<parvi_app_kit::menu::MenuRow> {
        use parvi_app_kit::menu::MenuRow;
        let r = &self.board.rows[row];
        let id = r.written_id(&self.board);
        let title = format!("{id}: {}", r.title);
        let mut rows = Vec::new();
        if self.board.projects[r.project].checkout.is_some() {
            let me = self.me.clone();
            rows.push(MenuRow::new("Start a session on it", move |_, cx| {
                let _ = me.update(cx, |w, cx| w.start_session_for(row, cx));
            }));
        }
        rows.extend([
            MenuRow::new("Copy id", move |_, cx| {
                parvi_app_kit::widgets::select::copy(id.clone(), cx)
            }),
            MenuRow::new("Copy id and title", move |_, cx| {
                parvi_app_kit::widgets::select::copy(title.clone(), cx)
            }),
        ]);
        if let Some(url) = r.url.clone() {
            rows.push(MenuRow::new("Open on GitHub", move |_, cx| {
                cx.open_url(&url)
            }));
        }
        rows
    }
    fn copy_prose(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.prose.borrow().copied() else {
            return false;
        };
        parvi_app_kit::widgets::select::copy(text, cx);
        true
    }
    /// The line that names the item shown to an agent.
    pub fn line(&self) -> Option<String> {
        Some(self.line_of(self.shown_row()?))
    }
    /// The line that names a row to an agent: the node's own words for the
    /// item it was asked for, else the board's.
    pub fn line_of(&self, row: usize) -> String {
        let item = &self.board.rows[row];
        self.card_line
            .as_ref()
            .filter(|(reference, _)| reference == &item.reference)
            .map(|(_, line)| line.clone())
            .unwrap_or_else(|| item.line(&self.board))
    }
    pub fn send(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(line) = self.line() {
            // The terminals choose the target by their own rule (the
            // terminal this item sits beside, else the last one sent to),
            // never by guessing from the item's folder.
            let _ = self.host.ask(
                Ask::Send {
                    reference: line,
                    target: None,
                },
                cx,
            );
        }
    }
    /// Asks the terminals page to start a session in the shown item's
    /// checkout.
    pub fn start_session(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.shown_row() {
            self.start_session_for(row, cx);
        }
    }
    /// Asks the terminals page to start a session in a row's checkout.
    pub fn start_session_for(&mut self, row: usize, cx: &mut Context<Self>) {
        let item = &self.board.rows[row];
        let project = &self.board.projects[item.project];
        let (Some(folder), Some(node)) = (
            project.checkout.clone(),
            self.node_for_project(item.project),
        ) else {
            return;
        };
        let _ = self.host.ask(
            Ask::Launch {
                node: NodeId(node),
                folder,
                name: session_name(&project.label, &item.id),
            },
            cx,
        );
    }

    /// A link in an item's text: a Bead id shows that Bead, `#n` its issue
    /// or pull request (or GitHub's page when not read), anything else
    /// opens in the browser.
    pub fn follow_link(&mut self, link: &str, project: usize, cx: &mut Context<Self>) {
        if let Some(id) = link.strip_prefix(prose::BEAD_LINK) {
            if let Some(row) = self.board.find_from(id, project) {
                self.follow_row(row, cx);
            }
        } else if let Some(n) = link.strip_prefix('#').and_then(|n| n.parse::<u32>().ok()) {
            let p = &self.board.projects[project];
            let reference = format!("gh:{}#{n}", p.path);
            match self
                .board
                .find(&reference)
                .or_else(|| self.board.pull(project, n))
            {
                Some(row) => self.follow_row(row, cx),
                None => {
                    if let Some(u) = &p.url {
                        cx.open_url(&format!("{u}/issues/{n}"));
                    }
                }
            }
        } else {
            cx.open_url(link);
        }
    }

    /// A family link: the row if the node read it, else nothing to open.
    pub fn follow(&mut self, reference: &str, cx: &mut Context<Self>) {
        if let Some(row) = self.board.find(reference) {
            self.follow_row(row, cx);
        }
    }
}

const SEARCH: &str = "Search…";
const PICK: &str = "Project…";

/// Every node's projects in one snapshot, in node order.
fn combined(snapshots: &BTreeMap<String, Arc<Snapshot>>) -> Snapshot {
    Snapshot {
        refreshed: 0,
        projects: snapshots
            .values()
            .flat_map(|s| s.projects.iter().cloned())
            .collect(),
    }
}

/// Suggest a tmux-safe name; the answering page resolves collisions.
fn session_name(project: &str, id: &str) -> String {
    let short = project.rsplit('/').next().unwrap_or(project);
    let id = id.trim_start_matches('#');
    let raw = if id.starts_with(short) {
        id.to_string()
    } else {
        format!("{short}-{id}")
    };
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect()
}
