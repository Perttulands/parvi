//! Work's table, filters and item card.
mod beside;
mod column;
mod fit;
mod graph_view;
mod natural;
mod report;
pub mod store;
mod strip;
#[cfg(test)]
mod tests;
mod tokens;
mod view;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, KeyDownEvent, UniformListScrollHandle,
    WeakEntity, Window,
};
use parvi_app_kit::ask::{Answers, Ask};
use parvi_app_kit::hub::now_ms;
use parvi_app_kit::widgets::field::{self, FieldEvent, TextField};
use parvi_app_kit::widgets::select::Selectable;
use parvi_app_kit::{Host, Mounted, Page, mount};
use parvi_core::NodeId;
use parvi_work_contract::{SNAPSHOT, Snapshot, WATCH};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use store::work::{self, Board, RowKey, Slice, Sliced, prose, when};

pub const NAME: &str = "work";
pub fn build(host: Host, window: &mut Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, Work::new)
}

pub struct Work {
    host: Host,
    me: WeakEntity<Work>,
    sessions: Option<Entity<parvi_terminals_contract::SessionList>>,
    focus: FocusHandle,
    search: Entity<TextField>,
    rail_search: Entity<TextField>,
    report: Option<Entity<report::Report>>,
    snapshots: BTreeMap<String, Snapshot>,
    board: Board,
    slice: Slice,
    sliced: Sliced,
    selected: Option<RowKey>,
    selected_node: Option<String>,
    card: bool,
    beside_from: Option<&'static str>,
    card_line: Option<String>,
    trail: Vec<(Option<String>, RowKey)>,
    prose: Rc<RefCell<Selectable>>,
    scroll: UniformListScrollHandle,
    wanted: Option<(String, Option<String>)>,
    drag_from: Option<i32>,
    pane: Rc<Cell<f32>>,
    columns: Vec<work::Column>,
}

impl Page for Work {
    const ID: &'static str = NAME;
    const TITLE: &'static str = "Work";
    const AREA: Option<&'static str> = Some(NAME);
    const KINDS: parvi_core::frame::Kinds = &[SNAPSHOT..=SNAPSHOT];
    const CHANNELS: &'static [u8] = &[WATCH];
    const ANSWERS: &'static [Answers] = &[Answers::Reveal("item")];
    fn frame(&mut self, node: &NodeId, _: u8, body: &[u8], _: &mut Window, cx: &mut Context<Self>) {
        if let Ok(s) = serde_json::from_slice::<Snapshot>(body) {
            self.snapshots.insert(node.as_str().into(), s);
            self.rebuild(cx);
        }
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
            parvi_app_kit::chords::Chord::new("i", "Close the table", move |_, cx| {
                let _ = close.update(cx, |work, cx| {
                    if let Some(from) = work.beside_from.take() {
                        work.host.close_beside(from, cx);
                    }
                });
            })
            .alt("i", Some(false))
            .everywhere(),
            parvi_app_kit::chords::Chord::new("b", "Work column", move |_, cx| {
                let _ = me.update(cx, |work, cx| work.toggle_column(cx));
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
        let search = cx.new(|cx| TextField::new(cx, "Search ids, titles, text…"));
        cx.subscribe_in(&search, window, |w: &mut Self, _, ev, window, cx| {
            w.search_event(*ev, window, cx)
        })
        .detach();
        let rail_search = cx.new(|cx| TextField::new(cx, "Search projects…"));
        cx.subscribe_in(
            &rail_search,
            window,
            |_: &mut Self, _, ev: &FieldEvent, _, cx| {
                if *ev == FieldEvent::Changed {
                    cx.notify();
                }
            },
        )
        .detach();
        let sessions = host.model::<parvi_terminals_contract::SessionList>(cx);
        Self {
            host,
            me: cx.weak_entity(),
            sessions,
            focus: cx.focus_handle(),
            search,
            rail_search,
            report: None,
            snapshots: BTreeMap::new(),
            board: Board::default(),
            slice: Slice::default(),
            sliced: Sliced::default(),
            selected: None,
            selected_node: None,
            card: false,
            beside_from: None,
            card_line: None,
            trail: Vec::new(),
            prose: Rc::default(),
            scroll: UniformListScrollHandle::new(),
            wanted: None,
            drag_from: None,
            pane: Rc::default(),
            columns: work::COLUMNS.to_vec(),
        }
    }
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let snapshot = Snapshot {
            projects: self
                .snapshots
                .values()
                .flat_map(|s| s.projects.clone())
                .collect(),
        };
        self.board = Board::from_snapshot(&snapshot, now_ms(), &when::local_offset);
        self.reslice();
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
            self.host.send(&node.id, vec![WATCH, 2], cx);
        }
    }
    pub fn reslice(&mut self) {
        self.sliced = self.slice.apply(&self.board);
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

    /// The selected row's place in the slice.
    fn cursor(&self) -> Option<usize> {
        let r = self.selected_row()?;
        self.sliced.rows.iter().position(|&x| x == r)
    }

    pub fn select(&mut self, row: usize, cx: &mut Context<Self>) {
        self.trail.clear();
        let selected = self.board.rows.get(row).map(RowKey::of);
        let selected_node = self
            .board
            .rows
            .get(row)
            .and_then(|r| self.node_for_project(r.project));
        if selected != self.selected || selected_node != self.selected_node {
            self.prose.borrow_mut().selection.clear();
        }
        self.selected = selected;
        self.selected_node = selected_node;
        // The selection brings its graph and its text beside the table.
        self.card = self.selected.is_some();
        if let Some(at) = self.cursor() {
            self.scroll.scroll_to_item(at, gpui::ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Opens the card on a row, reached from anywhere: its project and
    /// status are let through if the slice hid them.
    pub fn open(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(r) = self.board.rows.get(row) else {
            return;
        };
        let path = self.board.projects[r.project].path.clone();
        if !self.slice.keeps_project(&path) {
            self.slice.projects = None;
        }
        if !self.slice.keeps_status(r.status) {
            self.slice.statuses = None;
        }
        self.reslice();
        if !self.sliced.rows.contains(&row) {
            self.search.update(cx, |f, cx| f.set("", false, cx));
            self.slice.query.clear();
            self.reslice();
        }
        self.card = true;
        self.select(row, cx);
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let n = self.sliced.rows.len();
        if n == 0 {
            return;
        }
        let at = match self.cursor() {
            Some(c) => (c as isize + by).clamp(0, n as isize - 1) as usize,
            None if by < 0 => n - 1,
            None => 0,
        };
        self.select(self.sliced.rows[at], cx);
    }

    fn search_event(&mut self, ev: FieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            FieldEvent::Changed => {
                self.slice.query = self.search.read(cx).text.clone();
                self.reslice();
            }
            FieldEvent::Down | FieldEvent::Submit => {
                if self.cursor().is_none() {
                    self.step(1, cx);
                }
                if ev == FieldEvent::Submit {
                    self.card = self.selected.is_some();
                }
                window.focus(&self.focus, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// CHROTE's Beads keys: ↑↓ or j/k, Enter, Esc, `/`; Alt+S hands over
    /// and Alt+Left goes back along the trail.
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
            "enter" => {
                self.card = self.selected.is_some();
                true
            }
            "escape" if self.card => {
                self.let_go();
                true
            }
            "escape" if !self.slice.query.is_empty() => {
                self.search.update(cx, |f, cx| f.set("", false, cx));
                self.slice.query.clear();
                self.reslice();
                true
            }
            "/" => {
                field::focus(&self.search, window, cx);
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

    /// Opens a row reached from the card, the item in hand joining the
    /// trail (CHROTE's `followBeadFromCard`).
    pub fn follow_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let from = self
            .selected
            .clone()
            .filter(|_| self.card)
            .map(|key| (self.selected_node.clone(), key));
        let mut trail = std::mem::take(&mut self.trail);
        self.open(row, cx);
        let Some(from) = from else {
            return;
        };
        if self.selected.as_ref() != Some(&from.1) || self.selected_node != from.0 {
            trail.push(from);
        }
        self.trail = trail;
    }

    /// Back along the trail to the item the last id was followed from;
    /// nothing with nothing behind. An item gone from the board is passed.
    pub fn back(&mut self, cx: &mut Context<Self>) {
        while let Some((node, key)) = self.trail.pop() {
            if let Some(row) = self.find_on(&key.reference, node.as_deref()) {
                let rest = std::mem::take(&mut self.trail);
                self.open(row, cx);
                self.trail = rest;
                cx.notify();
                return;
            }
        }
    }

    fn row_menu(&self, row: usize) -> Vec<parvi_app_kit::menu::MenuRow> {
        use parvi_app_kit::menu::MenuRow;
        let r = &self.board.rows[row];
        let id = r.written_id(&self.board);
        let title = format!("{id}: {}", r.title);
        let line = r.line(&self.board);
        let host = self.host.clone();
        vec![
            MenuRow::new("Send…", move |_, cx| {
                let _ = host.ask(
                    Ask::Send {
                        reference: line.clone(),
                        target: None,
                    },
                    cx,
                );
            })
            .chord("Alt+S"),
            MenuRow::rule(),
            MenuRow::new("Copy id", move |_, cx| {
                parvi_app_kit::widgets::select::copy(id.clone(), cx)
            }),
            MenuRow::new("Copy id and title", move |_, cx| {
                parvi_app_kit::widgets::select::copy(title.clone(), cx)
            }),
        ]
    }
    fn copy_prose(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.card {
            return false;
        }
        let Some(text) = self.prose.borrow().copied() else {
            return false;
        };
        parvi_app_kit::widgets::select::copy(text, cx);
        true
    }
    pub fn send(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.selected_row() {
            let item = &self.board.rows[row];
            let line = self
                .card_line
                .clone()
                .unwrap_or_else(|| item.line(&self.board));
            let node = self.node_for_project(item.project);
            let folder = self.board.projects[item.project].checkout.as_deref();
            let target = self.sessions.as_ref().and_then(|list| {
                list.read(cx)
                    .sessions
                    .iter()
                    .find(|session| {
                        Some(session.node.as_str()) == node.as_deref()
                            && session
                                .cwd
                                .as_deref()
                                .zip(folder)
                                .is_some_and(|(a, b)| same_folder(a, b))
                    })
                    .map(|session| parvi_app_kit::ask::SessionTarget {
                        node: NodeId(session.node.clone()),
                        user: session.user.clone().unwrap_or_default(),
                        name: session.name.clone(),
                    })
            });
            let _ = self.host.ask(
                Ask::Send {
                    reference: line,
                    target,
                },
                cx,
            );
        }
    }
    /// Ask the terminals page to start a session in this item's checkout.
    pub fn start_session(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row() else {
            return;
        };
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

    pub fn let_go(&mut self) {
        self.trail.clear();
        self.card = false;
        self.selected = None;
        self.selected_node = None;
        self.prose.borrow_mut().selection.clear();
    }

    /// The days a drag across the strip covers so far, from where it began.
    pub fn drag_to(&mut self, day: i32, cx: &mut Context<Self>) {
        let from = *self.drag_from.get_or_insert(day);
        self.slice.range = Some((from.min(day), from.max(day)));
        self.reslice();
        cx.notify();
    }

    pub fn clear_range(&mut self, cx: &mut Context<Self>) {
        self.slice.range = None;
        self.drag_from = None;
        self.reslice();
        cx.notify();
    }

    /// A link in an item's text: a Bead id selects that Bead, `#n` its
    /// issue or pull request (or GitHub's page when not read), anything
    /// else opens in the browser.
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

    pub fn sort_by(&mut self, c: crate::store::work::Column, cx: &mut Context<Self>) {
        self.slice.sort_by(c);
        self.reslice();
        cx.notify();
    }

    pub fn open_url(&self, cx: &mut Context<Self>) {
        if let Some(u) = self
            .selected_row()
            .and_then(|r| self.board.rows[r].url.clone())
        {
            cx.open_url(&u);
        }
    }

    /// A family link: the row if the node read it, else nothing to open.
    pub fn follow(&mut self, reference: &str, cx: &mut Context<Self>) {
        if let Some(row) = self.board.find(reference) {
            self.follow_row(row, cx);
        }
    }
}

/// Two spellings of the same checkout, including a home-relative path.
fn same_folder(a: &str, b: &str) -> bool {
    let a = a.trim_end_matches(['/', '\\']);
    let b = b.trim_end_matches(['/', '\\']);
    a == b
        || (b.starts_with("~/") && a.ends_with(&b[1..]))
        || (a.starts_with("~/") && b.ends_with(&a[1..]))
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
