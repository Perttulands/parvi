//! The page itself: its stores, the nodes' sessions and screens, and the
//! one place that decides what a frame, an act or a chord does on it. What
//! opens over it (Send, Overview, the scrollback search, Peek, the rename
//! fields) is a layer on the app's stack, drawn by [`super::surface`].

use super::launcher::Launcher;
use super::surface::{self, Surface};
use crate::remote::Notice;
use crate::remotes::{PaneAt, Remotes};
use crate::store::sessions::{self, Key};
use crate::store::settings::{Kept, Settings};
use crate::store::workspace::{LinkState, TileState};
use crate::term::screen::Screen;
use crate::theme::size;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, IntoElement,
    Pixels, Point, Render, ScrollHandle, Task, WeakEntity, Window, px,
};
use parking_lot::Mutex;
use parvi_app_kit::Host;
use parvi_app_kit::chords::Chord;
use parvi_app_kit::host::Status;
use parvi_app_kit::menu::MenuRow;
use parvi_app_kit::settings::Place;
use parvi_app_kit::widgets::field::{self, FieldEvent, TextField};
use parvi_app_kit::{Answers, Ask, Page, Setting};
use parvi_core::NodeId;
use parvi_core::frame::Kinds;
use parvi_terminals_contract::{Launch, SessionList, decode_down};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

/// What a menu row does.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    RenameTab(usize),
    /// Tab, and how many windows it shows.
    Windows(usize, usize),
    NewTab,
    CloseTab(usize),
    ClearTab(usize),
    SavePreset,
    RestorePreset(usize),
    DeletePreset(usize),
    Reconnect,
    ClaimAll,
    Peek(Key),
    Send(Key),
    Attach(Key, usize),
    Unassign(Key),
    Rename(Key),
    /// The same harness again, in the session's folder (CHROTE's Copy).
    Copy(Key),
    Kill(Key),
    /// Kills the rail's picked rows that it shows.
    KillPicked,
    /// Takes a session's tag out of a window of the shown tab.
    Remove(usize, Key),
    /// Puts a session's tag on screen in its window.
    ShowTag(usize, Key),
    Claim(usize),
    Release(usize),
    Pane(usize, u32),
    WindowList(usize, Key),
    WindowPane(usize, Key, u32),
    History(usize),
    /// Makes an ended session again, under its name, in its windows.
    Restart(Key),
}

/// A row of one of the page's menus, as its tests read it: what it says
/// and the [`Act`] it does. The app's menu gets it as a [`MenuRow`].
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: String,
    pub chord: Option<String>,
    pub act: Option<Act>,
    /// Destroys something: the first click arms it, the second runs it.
    pub danger: bool,
    pub checked: bool,
    /// Offered and refused; its hint says why.
    pub refused: bool,
    /// A line under the label: what the row does, or why it is refused.
    pub hint: Option<String>,
    /// The rows it opens in its place.
    pub sub: Vec<MenuItem>,
}

impl MenuItem {
    pub fn new(label: impl Into<String>, act: Act) -> Self {
        Self {
            label: label.into(),
            chord: None,
            act: Some(act),
            danger: false,
            checked: false,
            refused: false,
            hint: None,
            sub: vec![],
        }
    }
    /// A row that opens `rows` in the menu's place; `state` reads beside it.
    pub fn sub(label: impl Into<String>, state: Option<String>, rows: Vec<MenuItem>) -> Self {
        let mut item = Self::note(label);
        item.chord = Some(match state {
            Some(s) => format!("{s} ›"),
            None => "›".into(),
        });
        item.sub = rows;
        item
    }
    /// A row that does nothing now, saying why.
    pub fn refused(mut self, why: impl Into<String>) -> Self {
        self.refused = true;
        self.hint = Some(why.into());
        self
    }
    pub fn hint(mut self, text: impl Into<String>) -> Self {
        self.hint = Some(text.into());
        self
    }
    /// A row that only says something.
    pub fn note(label: impl Into<String>) -> Self {
        let mut item = Self::rule();
        item.label = label.into();
        item
    }
    pub fn chord(mut self, c: &str) -> Self {
        self.chord = Some(c.into());
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
    pub fn rule() -> Self {
        Self {
            label: String::new(),
            chord: None,
            act: None,
            danger: false,
            checked: false,
            refused: false,
            hint: None,
            sub: vec![],
        }
    }

    /// The row for the app's menu: a click does its act on `page`.
    pub(super) fn row(self, page: &WeakEntity<Terminals>) -> MenuRow {
        let mut row = match self.act {
            Some(act) => {
                let page = page.clone();
                MenuRow::new(self.label, move |window, cx| {
                    let _ = page.update(cx, |p, cx| p.act(act.clone(), window, cx));
                })
            }
            None => MenuRow::note(self.label),
        };
        row.chord = self.chord;
        row.danger = self.danger;
        row.checked = self.checked;
        row.refused = self.refused;
        row.hint = self.hint;
        row.sub = self.sub.into_iter().map(|i| i.row(page)).collect();
        row
    }
}

/// A window's read-only view of its pane's history, while scrolled back.
pub struct History {
    pub pane: PaneAt,
    pub screen: Arc<Mutex<Screen>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HistoryTarget {
    Window { index: usize, line: Option<usize> },
    Peek,
}

/// Peek: one session, observed, never sized.
pub struct Peek {
    pub key: Key,
    pub pane: Option<u32>,
    /// Opened at a search result: the history at that line.
    pub history: Option<Arc<Mutex<Screen>>>,
    pub line: Option<u32>,
    pub loading: bool,
}

/// A terminal grid: columns, rows.
pub type Grid = (u16, u16);

/// Where each window's terminal was last painted, for the mouse.
#[derive(Clone, Copy, Default)]
pub struct Geo {
    pub body: Option<Bounds<Pixels>>,
    pub origin: Option<Point<Pixels>>,
    pub cell: Option<gpui::Size<Pixels>>,
    pub rows: usize,
    pub cols: usize,
    pub display_offset: usize,
}

/// A launch awaiting its node's answer, including the status it started with.
pub struct PendingLaunch {
    launch: Launch,
    window: Option<usize>,
    tab: usize,
    status_revision: u64,
}

pub struct Terminals {
    pub host: Host,
    pub me: WeakEntity<Self>,
    pub focus: FocusHandle,
    pub term_focus: FocusHandle,
    /// Peek needs its own focus while another surface is underneath it.
    pub peek_focus: FocusHandle,
    pub fleet: Remotes,
    pub settings: Settings,
    /// The fleet as the app last saw it, by node.
    pub places: BTreeMap<String, Place>,
    /// The page is the one shown.
    pub shown: bool,
    /// What is open over the page, bottom first, as this page opened it.
    pub surfaces: Vec<Surface>,
    /// Status lines said since the page last handed them to the app.
    said: Vec<Status>,
    pub geo: Rc<RefCell<[Geo; 4]>>,
    /// The grid each window's body would hold at the terminal font,
    /// as last painted: what a launch into it asks the node for (#161).
    pub launch_grids: Rc<RefCell<[Option<Grid>; 4]>>,
    /// The window a drag selection started in.
    pub selecting: Option<usize>,
    /// Lines a selection drag held outside its grid scrolls each tick, and
    /// the ticks while it does.
    pub drag_scroll: i32,
    pub drag_scroll_task: Option<Task<()>>,
    pub mouse_down_in: Option<(usize, u8)>,
    /// The cell (window, column, row on screen) the pointer rests on, when
    /// a link is drawn there.
    pub link_hover: Option<(usize, usize, usize)>,
    pub histories: HashMap<usize, History>,
    /// History requests in flight: request id → window, or Peek.
    pub history_wanted: HashMap<u32, HistoryTarget>,
    pub peek: Option<Peek>,
    pub launchers: Vec<Entity<Launcher>>,
    pub rail_search: Entity<TextField>,
    pub send_note: Entity<TextField>,
    pub send_query: Entity<TextField>,
    pub send_target: Option<Key>,
    pub send_cursor: usize,
    pub send: super::send::Draft,
    pub find_query: Entity<TextField>,
    pub find_cursor: usize,
    pub rename_field: Entity<TextField>,
    pub overview_cursor: usize,
    /// Number of overview cards fitting in the last painted row.
    pub overview_columns: Rc<Cell<usize>>,
    pub overview_scroll: ScrollHandle,
    /// The terminal tabs' strip, and the tab it last brought into view.
    pub tab_scroll: ScrollHandle,
    pub tab_scrolled: Option<usize>,
    pub hover_row: Option<String>,
    /// Launches asked of a node, by node and request: the recipe, the window
    /// it goes to and that window's tab.
    pub launches: HashMap<(String, u32), PendingLaunch>,
    /// Changes whenever this page publishes a status: older async successes
    /// cannot replace a later status the operator has already been shown.
    pub status_revision: u64,
    /// Kills asked of nodes, answered together (`store::selection`).
    pub kills: crate::store::selection::Kills,
    /// Rail rows picked to kill together; on this device only, never kept.
    pub pick: crate::store::selection::Pick,
    /// The rail's rows as last drawn, in order: what a Shift+click range
    /// and Kill N count from.
    pub rail_shown: Vec<Key>,
    /// The rail takes focus on a pick, to hold Delete and Escape.
    pub rail_focus: FocusHandle,
    /// Mouse changes asked of nodes, by node and request: the device's name.
    pub mice: HashMap<(String, u32), String>,
    /// Renames asked of a node, by node and request: the session's key as
    /// it was, and its new name.
    pub renames: HashMap<(String, u32), (Key, String)>,
    pub dragging_over: Option<usize>,
    /// The seam a drag is over: after which window of the shown tab.
    pub seam_over: Option<usize>,
    /// Where the last menu a row was picked from stood.
    pub menu_at: Point<Pixels>,
    /// Ended sessions being made again, until their node answers.
    pub restarting: HashSet<Key>,
    /// How many rows Peek shows at the font size, measured as the window draws.
    pub peek_rows: usize,
    /// The agent event each session was last told seen for, by key text.
    pub seen_sent: HashMap<String, u64>,
    /// The operator pressed a key or a button on this page, or showed a
    /// session, since it last drew: he is looking at it.
    pub acted: bool,
    /// Wakes the page when the network thread fed a screen.
    _woken: Task<()>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Terminals {
    pub fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut said = vec![];
        let settings = match host.setting::<Settings>(cx) {
            Ok(s) => s.mend(),
            Err(e) => {
                said.push(Status::failure(format!(
                    "The terminals' settings did not read ({}): {}",
                    e.code, e.message
                )));
                Settings::default()
            }
        };
        let field = |cx: &mut Context<Self>, p: &str| cx.new(|cx| TextField::new(cx, p));
        let rail_search = field(cx, "Search sessions…");
        let send_note = cx.new(|cx| {
            TextField::new(cx, "Message to send")
                .multiline()
                .attachments()
        });
        let send_query = field(cx, "Find a session…");
        let find_query = field(cx, "Search every scrollback…");
        let rename_field = field(cx, "Name");
        let weak = cx.weak_entity();
        // One per window, and the Send drawer's last.
        let launchers = (0..=super::launcher::SEND)
            .map(|i| cx.new(|cx| Launcher::new(i, weak.clone(), host.clone(), window, cx)))
            .collect();
        let fleet = Remotes::default();
        let (wake, woken) = async_channel::bounded(1);
        host.tap(fleet.tap(wake), cx);
        let woken = cx.spawn(async move |this, cx| {
            while woken.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        // What the page asks of the nodes, and says, leaves as it redraws.
        cx.observe_self(|p, cx| p.flush(cx)).detach();
        let places = host.places(cx);
        let mut s = Self {
            host,
            me: weak,
            focus: cx.focus_handle(),
            term_focus: cx.focus_handle(),
            peek_focus: cx.focus_handle(),
            fleet,
            settings,
            places,
            shown: false,
            surfaces: vec![],
            said,
            geo: Rc::new(RefCell::new([Geo::default(); 4])),
            launch_grids: Rc::default(),
            selecting: None,
            drag_scroll: 0,
            drag_scroll_task: None,
            mouse_down_in: None,
            link_hover: None,
            histories: HashMap::new(),
            history_wanted: HashMap::new(),
            peek: None,
            launchers,
            rail_search,
            send_note,
            send_query,
            send_target: None,
            send_cursor: 0,
            send: Default::default(),
            find_query,
            find_cursor: 0,
            rename_field,
            overview_cursor: 0,
            overview_columns: Rc::new(Cell::new(1)),
            overview_scroll: ScrollHandle::new(),
            tab_scroll: ScrollHandle::new(),
            tab_scrolled: None,
            hover_row: None,
            launches: HashMap::new(),
            status_revision: 0,
            kills: Default::default(),
            pick: Default::default(),
            rail_shown: vec![],
            rail_focus: cx.focus_handle(),
            mice: HashMap::new(),
            renames: HashMap::new(),
            dragging_over: None,
            seam_over: None,
            menu_at: Point::default(),
            restarting: HashSet::new(),
            peek_rows: 0,
            seen_sent: HashMap::new(),
            acted: false,
            _woken: woken,
        };
        s.subscribe_fields(window, cx);
        // Nodes the app already reaches were opened before the page was made.
        for n in s.host.nodes(cx) {
            if let Some(state) = s.host.link(&n.id, cx) {
                s.link_changed(&n.id, Some(state), window, cx);
            }
        }
        s
    }

    fn subscribe_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.subscribe_in(
            &self.rail_search.clone(),
            window,
            |s, _, ev, window, cx| match ev {
                FieldEvent::Changed => cx.notify(),
                FieldEvent::Submit => {
                    // Enter on a search peeks at the first session found.
                    let q = s.rail_search.read(cx).text.clone();
                    if let Some(k) = sessions::search(&s.fleet.all, &q)
                        .first()
                        .map(|x| Key::of(x))
                    {
                        s.open_peek(k, None, window, cx);
                    }
                }
                _ => {}
            },
        )
        .detach();
        cx.subscribe_in(
            &self.send_note.clone(),
            window,
            |s, _, ev, window, cx| match ev {
                FieldEvent::Submit => s.send_now(true, window, cx),
                FieldEvent::SubmitAlt => s.send_now(false, window, cx),
                FieldEvent::Changed => cx.notify(),
                _ => {}
            },
        )
        .detach();
        cx.subscribe_in(
            &self.send_note.clone(),
            window,
            |s, _, ev: &field::AttachmentEvent, _, cx| {
                s.send_clipboard(ev.0.clone(), cx);
            },
        )
        .detach();
        cx.subscribe_in(&self.send_query.clone(), window, |s, _, ev, window, cx| {
            let q = s.send_query.read(cx).text.clone();
            let n = s.send_targets(&q).len();
            match ev {
                FieldEvent::Changed => s.send_cursor = 0,
                FieldEvent::Up => s.send_cursor = s.send_cursor.saturating_sub(1),
                FieldEvent::Down => s.send_cursor = (s.send_cursor + 1).min(n.saturating_sub(1)),
                FieldEvent::Submit | FieldEvent::Tab => {
                    let t = s.send_targets(&q);
                    if let Some(x) = t.get(s.send_cursor) {
                        let target = Key::of(x);
                        if s.send_target.as_ref() != Some(&target) {
                            s.send.pane = None;
                        }
                        s.send.new_agent = false;
                        s.send_target = Some(target);
                    }
                    s.send_query.update(cx, |f, cx| f.set("", false, cx));
                    field::focus(&s.send_note, window, cx);
                }
                _ => {}
            }
            cx.notify();
        })
        .detach();
        cx.subscribe_in(&self.find_query.clone(), window, |s, _, ev, window, cx| {
            let n = s.fleet.found().map_or(0, |(m, _)| m.len());
            match ev {
                FieldEvent::Changed => {
                    let q = s.find_query.read(cx).text.clone();
                    s.find_cursor = 0;
                    s.fleet.search(&q);
                }
                FieldEvent::Up => s.find_cursor = s.find_cursor.saturating_sub(1),
                FieldEvent::Down => s.find_cursor = (s.find_cursor + 1).min(n.saturating_sub(1)),
                FieldEvent::Submit => s.open_match(s.find_cursor, window, cx),
                _ => {}
            }
            cx.notify();
        })
        .detach();
        cx.subscribe_in(
            &self.rename_field.clone(),
            window,
            |s, _, ev, window, cx| {
                if !matches!(ev, FieldEvent::Submit) {
                    return;
                }
                let name = s.rename_field.read(cx).text.clone();
                match s.surfaces.last().cloned() {
                    Some(Surface::RenameTab(_)) => s.commit_tab_name(window, cx),
                    Some(Surface::RenameSession(k)) => {
                        s.close(surface::RENAME_SESSION, cx);
                        s.rename_session(k, name.trim());
                    }
                    Some(Surface::PresetName(_)) => {
                        s.close(surface::PRESET_NAME, cx);
                        match s.settings.workspace.save_preset(&name) {
                            Ok(Some(n)) => s.report(&format!("Layout '{n}' saved"), false, None),
                            Ok(None) => {}
                            Err(why) => s.report(&why, true, None),
                        }
                        s.save(cx);
                    }
                    _ => {}
                }
            },
        )
        .detach();
        // A tab's name is kept when its field loses focus, as CHROTE's input
        // commits on blur; Escape closes it first, keeping the old name.
        let rename_focus = self.rename_field.read(cx).focus_handle(cx);
        cx.on_blur(&rename_focus, window, |s, window, cx| {
            if matches!(s.surfaces.last(), Some(Surface::RenameTab(_))) {
                s.commit_tab_name(window, cx);
            }
        })
        .detach();
    }

    // ---- the nodes ----

    /// Hands the app what the page asked of the nodes and said since.
    pub fn flush(&mut self, cx: &mut App) {
        for (node, frames) in self.fleet.outbox() {
            let node = NodeId(node);
            for frame in frames {
                self.host.send(&node, frame, cx);
            }
        }
        for status in std::mem::take(&mut self.said) {
            self.host.report(status, cx);
        }
    }

    /// The link to `node` changed; `None` when the app no longer follows it.
    pub fn link_changed(
        &mut self,
        node: &NodeId,
        state: Option<LinkState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let first = self.host.nodes(cx).first().map(|n| n.id.0.clone());
        if state == Some(LinkState::Open) {
            // A look told while the link was down may not have arrived.
            self.seen_sent.clear();
        }
        self.places = self.host.places(cx);
        for n in self.fleet.link(node.as_str(), state, first.as_deref()) {
            self.notice(node.as_str(), n, window, cx);
        }
        self.after_news(cx);
    }

    /// A frame of the terminals' kinds from `node`.
    pub fn frame_from(
        &mut self,
        node: &NodeId,
        kind: u8,
        body: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(down) = decode_down(kind, body) else {
            log::warn!("a terminals frame of kind {kind:#04x} from {node} did not read");
            return;
        };
        for n in self.fleet.frame(node.as_str(), down) {
            self.notice(node.as_str(), n, window, cx);
        }
        self.after_news(cx);
    }

    /// What follows anything a node said: its sessions remembered, a
    /// launched session's message delivered, and the panes watched.
    fn after_news(&mut self, cx: &mut Context<Self>) {
        self.remember_sessions(cx);
        self.send_waited(cx);
        self.sync(cx);
        cx.notify();
    }

    /// Tells other pages the sessions as now heard ([`SessionList`]).
    pub(super) fn publish_sessions(&mut self, cx: &mut Context<Self>) {
        let list = SessionList {
            sessions: self.fleet.all.clone(),
            focused: self
                .focused_key()
                .map(|k| parvi_terminals_contract::FocusedSession {
                    node: k.node,
                    user: k.user,
                    name: k.name,
                }),
        };
        if let Err(e) = self.host.publish(list, cx) {
            self.report(&e.message, true, None);
        }
    }

    /// Where a node is, as it said last.
    pub fn where_is(&self, node: &str) -> Option<Place> {
        self.places.get(node).cloned()
    }

    /// When a node that does not answer now was last heard, as a clock
    /// time; `None` while it answers or if it never went.
    pub fn away_since(&self, node: &str) -> Option<String> {
        if self
            .fleet
            .get(node)
            .is_some_and(|c| c.state == LinkState::Open)
        {
            return None;
        }
        self.places.get(node)?.seen.map(parvi_app_kit::clock::hhmm)
    }

    /// Keeps each node's session names, so an away device keeps its rows
    /// and windows, named, until it answers again; nodes the app no longer
    /// knows go.
    fn remember_sessions(&mut self, cx: &mut Context<Self>) {
        let kept = &mut self.settings.kept;
        let before = kept.clone();
        for c in self.fleet.clients().iter().filter(|c| c.known) {
            let list = c
                .sessions
                .iter()
                .map(|s| Kept {
                    name: s.name.clone(),
                    user: s.user.clone().unwrap_or_default(),
                })
                .collect();
            kept.insert(c.node.clone(), list);
        }
        if !self.places.is_empty() {
            kept.retain(|n, _| self.places.contains_key(n));
        }
        let changed = *kept != before;
        let before_restarts = self.settings.restarts.clone();
        for session in &self.fleet.all {
            let key = Key::of(session).text();
            let record = self.settings.restarts.entry(key).or_default();
            if record.id != session.id {
                *record = crate::store::settings::Restart {
                    id: session.id.clone(),
                    ..Default::default()
                };
            }
            if record.harness.is_none()
                && let Some(cwd) = &session.cwd
                && !cwd.is_empty()
            {
                record.folder.clone_from(cwd);
            }
        }
        if changed || self.settings.restarts != before_restarts {
            self.save(cx);
        }
    }

    fn notice(&mut self, node: &str, n: Notice, window: &mut Window, cx: &mut Context<Self>) {
        match n {
            Notice::Reported(list) => self.reported(list, window, cx),
            Notice::Waiting(names) => {
                let text = match names.as_slice() {
                    [one] => format!("{one} is waiting"),
                    many => format!("{} sessions are waiting", many.len()),
                };
                self.report(&text, false, None);
            }
            Notice::Launched { id, ok, text } => {
                self.send_launched(node, id, ok, &text, cx);
                let Some(PendingLaunch {
                    launch,
                    window: window_index,
                    tab,
                    status_revision,
                }) = self.launches.remove(&(node.to_string(), id))
                else {
                    return;
                };
                let name = launch.name.clone();
                // A restart answered: its windows were bound all along.
                self.restarting
                    .retain(|k| !(k.node == node && k.name == name));
                if ok {
                    let key = Key {
                        node: node.to_string(),
                        user: launch.user.clone(),
                        name: name.clone(),
                    };
                    self.settings.restarts.insert(
                        key.text(),
                        crate::store::settings::Restart {
                            id: text.split('\0').next().unwrap_or_default().to_string(),
                            folder: launch.folder.clone(),
                            harness: Some(launch.harness.clone()),
                            flags: launch.flags.clone(),
                            notify: launch.notify,
                        },
                    );
                    self.save(cx);
                    if let Some(w) = window_index {
                        let node = node.to_string();
                        let user = self
                            .launchers
                            .get(w)
                            .map(|l| l.read(cx).user.clone())
                            .unwrap_or_default();
                        let key = Key {
                            node,
                            user,
                            name: name.clone(),
                        };
                        self.settings.workspace.bind_in(tab, w, key, None);
                        self.save(cx);
                    }
                    // After a NUL, what did not go as asked: no completion
                    // notices, or a harness that did not start.
                    match text.split_once('\0') {
                        Some((_, warning)) => {
                            self.report(&format!("Launched {name}, but {warning}"), true, None)
                        }
                        None if self.status_revision == status_revision => {
                            self.report(&format!("Launched {name}"), false, None)
                        }
                        None => {}
                    }
                    window.focus(&self.term_focus, cx);
                } else {
                    let why = if text.is_empty() {
                        "the node refused".to_string()
                    } else {
                        text
                    };
                    self.report(&format!("Could not launch {name}: {why}"), true, None);
                }
                if let Some(w) = window_index
                    && let Some(l) = self.launchers.get(w)
                {
                    l.update(cx, |l, cx| l.done(cx));
                }
            }
            Notice::Deleted { id, ok, text } => self.killed(node, id, ok, &text),
            Notice::Moused { id, ok, text } => self.moused(node, id, ok, &text),
            Notice::Renamed { id, ok, text } => {
                let Some((old, name)) = self.renames.remove(&(node.to_string(), id)) else {
                    return;
                };
                if ok {
                    self.settings.workspace.rename_session(&old, &name);
                    let new = Key {
                        name: name.clone(),
                        ..old.clone()
                    };
                    if let Some(record) = self.settings.restarts.remove(&old.text()) {
                        self.settings.restarts.insert(new.text(), record);
                    }
                    for k in [
                        self.peek.as_mut().map(|p| &mut p.key),
                        self.send_target.as_mut(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if *k == old {
                            *k = new.clone();
                        }
                    }
                    self.save(cx);
                    self.report(&format!("Renamed {} to {name}", old.name), false, None);
                } else {
                    self.report(
                        &format!("Could not rename {}: {text}", old.name),
                        true,
                        None,
                    );
                }
            }
            Notice::Delivered { id, outcome } => self.send_delivered(id, outcome, cx),
            Notice::History { id, pane, text } => {
                let at = PaneAt {
                    node: node.to_string(),
                    pane,
                };
                self.history_arrived(id, at, text, cx)
            }
            Notice::ClaimRefused(why) => {
                self.report(&format!("tmux kept its size: {why}"), true, None)
            }
        }
    }

    /// Subscriptions follow the bindings (every tab, so a tab switch shows
    /// a live screen at once) and Peek.
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        let bindings = self
            .settings
            .workspace
            .tabs
            .iter()
            .flat_map(|tab| &tab.windows)
            .flat_map(|window| &window.sessions)
            .map(|binding| binding.key.clone())
            .collect();
        self.fleet.retain_ended_screens(&bindings);
        let mut panes: HashMap<String, HashSet<u32>> = HashMap::new();
        let ws = &self.settings.workspace;
        for b in ws.on_screen() {
            if let Some(s) = self.fleet.session(&b.key)
                && let Some(p) = s
                    .panes
                    .iter()
                    .find(|p| Some(p.id) == b.pane)
                    .or(s.panes.first())
            {
                panes.entry(b.key.node.clone()).or_default().insert(p.id);
            }
        }
        if let Some(p) = &mut self.peek
            && let Some(s) = sessions::find(&self.fleet.all, &p.key)
        {
            let pane = p
                .pane
                .filter(|id| s.panes.iter().any(|x| x.id == *id))
                .or(s.panes.first().map(|x| x.id));
            p.pane = pane;
            if let Some(id) = pane {
                panes.entry(p.key.node.clone()).or_default().insert(id);
            }
        }
        // A window whose session went stops sizing it.
        self.fleet.watch(&panes);
        self.publish_sessions(cx);
    }

    // ---- reporting ----

    /// Says something on the app's status line, with a button that does
    /// `action` on this page.
    pub fn report(&mut self, text: &str, failure: bool, action: Option<(String, Act)>) {
        self.status_revision = self.status_revision.wrapping_add(1);
        let mut status = if failure {
            Status::failure(text)
        } else {
            Status::new(text)
        };
        if let Some((label, act)) = action {
            let page = self.me.clone();
            status = status.action(label, move |window, cx| {
                let _ = page.update(cx, |p, cx| p.act(act.clone(), window, cx));
            });
        }
        self.said.push(status);
    }

    pub(super) fn track_launch(
        &mut self,
        node: &str,
        id: u32,
        launch: Launch,
        window: Option<usize>,
        tab: usize,
    ) {
        self.launches.insert(
            (node.to_string(), id),
            PendingLaunch {
                launch,
                window,
                tab,
                status_revision: self.status_revision,
            },
        );
    }

    pub fn toast(&mut self, text: &str, cx: &mut Context<Self>) {
        self.host.toast(text, cx);
    }

    /// Keeps the page's settings in the app's.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.host.set_setting(&self.settings, cx) {
            self.report(
                &format!("The terminals' settings were not kept: {}", e.message),
                true,
                None,
            );
        }
    }

    // ---- surfaces ----

    pub fn is_open(&self, name: &str) -> bool {
        self.surfaces.iter().any(|s| s.name() == name)
    }

    /// Opens `s` over the page, as a layer on the app's stack.
    pub fn open(&mut self, s: Surface, window: &mut Window, cx: &mut Context<Self>) {
        let name = s.name();
        self.surfaces.retain(|x| x.name() != name);
        self.surfaces.push(s.clone());
        let focus = match &s {
            Surface::Send => {
                if self.send_target.is_none() {
                    self.send_query.focus_handle(cx)
                } else {
                    self.send_note.focus_handle(cx)
                }
            }
            Surface::Overview => {
                self.host
                    .channel(parvi_terminals_contract::OVERVIEW, true, cx);
                self.overview_cursor = 0;
                self.overview_scroll.scroll_to_item(1);
                self.focus.clone()
            }
            Surface::Find => {
                self.find_cursor = 0;
                self.find_query
                    .update(cx, |f, cx| f.set(&f.text.clone(), true, cx));
                self.find_query.focus_handle(cx)
            }
            Surface::Peek => self.peek_focus.clone(),
            Surface::RenameTab(_) | Surface::RenameSession(_) | Surface::PresetName(_) => {
                self.rename_field.focus_handle(cx)
            }
        };
        let view = surface::view(self.me.clone(), name, cx);
        let page = self.me.clone();
        let layer = parvi_app_kit::host::Layer::new(name, view)
            .cover(s.cover())
            .focus(focus.clone())
            .on_dismiss(move |window, cx| {
                let _ = page.update(cx, |p, cx| p.closed(name, window, cx));
            });
        self.host.layer(layer, cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Closes the surface named `name`, and whatever opened above it.
    pub fn close(&mut self, name: &'static str, cx: &mut Context<Self>) {
        self.host.close_layer(name, cx);
    }

    /// The app closed the surface named `name`.
    fn closed(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.surfaces.iter().position(|s| s.name() == name) else {
            return;
        };
        let s = self.surfaces.remove(i);
        match s {
            Surface::Overview => {
                self.host
                    .channel(parvi_terminals_contract::OVERVIEW, false, cx);
                self.fleet.overview_off();
            }
            Surface::Peek => {
                self.peek = None;
                self.sync(cx);
            }
            Surface::Send => {
                self.send_closed(window, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Opens the app's menu at `at` with these rows.
    pub fn menu(
        &mut self,
        items: Vec<MenuItem>,
        at: Point<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_at = at;
        let rows = items.into_iter().map(|i| i.row(&self.me)).collect();
        self.host.menu(rows, at, cx);
    }

    // ---- actions ----

    pub fn act(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        let ws = &mut self.settings.workspace;
        match act {
            Act::RenameTab(i) => {
                let name = ws
                    .tabs
                    .get(i)
                    .and_then(|t| t.name.clone())
                    .unwrap_or_default();
                self.rename_field.update(cx, |f, cx| f.set(&name, true, cx));
                self.open(Surface::RenameTab(i), window, cx);
                return;
            }
            Act::Windows(tab, n) => {
                ws.set_windows(tab, n);
                if tab == ws.active {
                    self.histories.retain(|w, _| *w < n);
                }
            }
            Act::NewTab => {
                if !ws.add_tab() {
                    self.report("Six terminal tabs is the most.", false, None);
                }
                self.host.show(cx);
            }
            Act::CloseTab(i) => {
                ws.close_tab(i);
                self.histories.clear();
            }
            Act::ClearTab(i) => {
                if i == ws.active {
                    let shown: Vec<PaneAt> = self.window_panes().into_iter().flatten().collect();
                    for p in shown {
                        self.fleet.release(&p.node, p.pane);
                    }
                    self.histories.clear();
                }
                self.settings.workspace.clear(i);
            }
            Act::SavePreset => {
                self.rename_field.update(cx, |f, cx| f.set("", false, cx));
                self.open(Surface::PresetName(self.menu_at), window, cx);
                return;
            }
            Act::RestorePreset(i) => {
                if let Some(name) = ws.restore_preset(i) {
                    self.histories.clear();
                    self.report(&format!("Layout '{name}' loaded"), false, None);
                }
            }
            Act::DeletePreset(i) => {
                if let Some(name) = ws.delete_preset(i) {
                    self.report(&format!("Preset '{name}' deleted"), false, None);
                }
            }
            Act::Reconnect => {
                self.host.redial(cx);
                self.report("Reconnecting…", false, None);
            }
            Act::ClaimAll => {
                let panes = self.window_panes();
                let n = panes.iter().flatten().count();
                for (i, p) in panes.iter().enumerate() {
                    if let Some(p) = p {
                        self.claim_window(i, p);
                    }
                }
                self.report(
                    &format!(
                        "Claimed {n} {}",
                        if n == 1 { "session" } else { "sessions" }
                    ),
                    false,
                    None,
                );
            }
            Act::Peek(k) => {
                self.open_peek(k, None, window, cx);
                return;
            }
            Act::Send(k) => {
                self.open_send(Some(k), window, cx);
                return;
            }
            Act::Attach(k, w) => self.attach(k, w, window, cx),
            Act::Unassign(k) => {
                self.settings.workspace.unbind_everywhere(&k);
                if let Some(s) = self.fleet.session(&k) {
                    let panes: Vec<u32> = s.panes.iter().map(|p| p.id).collect();
                    for p in panes {
                        self.fleet.release(&k.node, p);
                    }
                }
            }
            Act::Rename(k) => {
                self.rename_field
                    .update(cx, |f, cx| f.set(&k.name, true, cx));
                self.open(Surface::RenameSession(k), window, cx);
                return;
            }
            Act::Copy(k) => self.copy_session(&k),
            Act::Kill(k) => self.kill_sessions(&[k], false),
            Act::KillPicked => {
                let targets = self.pick_targets();
                self.kill_sessions(&targets, true);
            }
            Act::Remove(w, k) => {
                if self.settings.workspace.shown(w).is_some_and(|b| b.key == k) {
                    if let Some(Some(p)) = self.window_panes().get(w) {
                        self.fleet.release(&p.node, p.pane);
                    }
                    self.histories.remove(&w);
                }
                self.settings.workspace.unbind(w, &k);
            }
            Act::ShowTag(w, k) => {
                if let Some(Some(p)) = self.window_panes().get(w) {
                    self.fleet.release(&p.node, p.pane);
                }
                if self.settings.workspace.show_tag(w, &k) {
                    self.histories.remove(&w);
                }
                self.settings.workspace.focus = w;
            }
            Act::Claim(w) => {
                if let Some(Some(p)) = self.window_panes().get(w) {
                    self.claim_window(w, p);
                }
            }
            Act::Release(w) => {
                if let Some(Some(p)) = self.window_panes().get(w) {
                    self.fleet.give_back(&p.node, p.pane);
                }
            }
            Act::Pane(w, p) => {
                self.settings.workspace.choose_pane(w, p);
                self.histories.remove(&w);
            }
            Act::WindowList(w, key) => self.window_list(w, key, cx),
            Act::WindowPane(w, key, pane) => {
                if self
                    .settings
                    .workspace
                    .shown(w)
                    .is_some_and(|b| b.key == key)
                    && self
                        .fleet
                        .session(&key)
                        .is_some_and(|s| s.panes.iter().any(|p| p.id == pane))
                {
                    self.settings.workspace.choose_pane(w, pane);
                    self.histories.remove(&w);
                }
            }
            Act::History(w) => self.open_history(w, cx),
            Act::Restart(k) => self.restart(k),
        }
        self.sync(cx);
        self.save(cx);
        cx.notify();
    }

    // ---- sessions ----

    pub fn focused_key(&self) -> Option<Key> {
        let ws = &self.settings.workspace;
        ws.shown(ws.focus).map(|b| b.key.clone())
    }

    /// The pane each window of the active tab shows, if live.
    pub fn window_panes(&self) -> Vec<Option<PaneAt>> {
        self.settings
            .workspace
            .windows()
            .iter()
            .map(|w| match self.fleet.tile(w.shown()) {
                TileState::Live { session, pane } => Some(PaneAt {
                    node: session.node.clone(),
                    pane,
                }),
                _ => None,
            })
            .collect()
    }

    /// Sizes the session at `at` to window `w`; false if the window has not
    /// been drawn, so its size is unknown.
    pub(super) fn claim_window(&mut self, w: usize, at: &PaneAt) -> bool {
        let g = self.geo.borrow()[w];
        if let (Some(body), Some(cell)) = (g.body, g.cell) {
            let m = crate::term::paint::Metrics {
                font_size: px(self.settings.font_size),
                cell,
            };
            let (cols, rows) = crate::term::paint::grid_for(body.size, m);
            self.fleet.claim_anew(&at.node, at.pane, cols, rows);
            return true;
        }
        false
    }

    /// Keeps the name typed in the tab bar for the tab being renamed.
    fn commit_tab_name(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Surface::RenameTab(i)) = self.surfaces.last().cloned() {
            let name = self.rename_field.read(cx).text.clone();
            self.settings.workspace.rename_tab(i, &name);
            self.close(surface::RENAME_TAB, cx);
            self.save(cx);
        }
    }

    /// Makes an ended session again under the same name and user. A witnessed
    /// launch keeps its recipe; other sessions use a shell in their last folder.
    fn restart(&mut self, k: Key) {
        if self.restarting.contains(&k) {
            return;
        }
        let ws = &self.settings.workspace;
        let target = ws
            .bound(&k)
            .filter(|(tab, _)| *tab == ws.active)
            .map_or(ws.focus, |(_, w)| w);
        let mut launch = restart_launch(&k, self.settings.restarts.get(&k.text()));
        launch.size = self.launch_grids.borrow()[target];
        match self.fleet.launch_on(&k.node, launch.clone()) {
            Some(id) => {
                let tab = self.settings.workspace.active;
                self.report(&format!("Restarting {}…", k.name), false, None);
                self.track_launch(&k.node, id, launch, None, tab);
                self.restarting.insert(k);
            }
            None => self.report(
                &format!("Could not restart {}: its device is away", k.name),
                true,
                None,
            ),
        }
    }

    /// Asks the session's node to rename it; its windows follow once the
    /// node answers. An empty or unchanged name changes nothing.
    fn rename_session(&mut self, k: Key, name: &str) {
        if name.is_empty() || name == k.name {
            return;
        }
        let Some(s) = self.fleet.session(&k).cloned() else {
            self.report(
                &format!("Could not rename {}: it is gone", k.name),
                true,
                None,
            );
            return;
        };
        match self.fleet.rename(&k.node, &s.id, name) {
            Some(id) => {
                self.report(&format!("Renaming {}…", k.name), false, None);
                self.renames
                    .insert((k.node.clone(), id), (k, name.to_string()));
            }
            None => self.report(
                &format!("Could not rename {}: its device is away", k.name),
                true,
                None,
            ),
        }
    }

    pub fn attach(&mut self, k: Key, w: usize, window: &mut Window, cx: &mut Context<Self>) {
        let ws = &mut self.settings.workspace;
        if w >= ws.windows().len() {
            return;
        }
        if let Some(Some(p)) = self.window_panes().get(w) {
            self.fleet.release(&p.node, p.pane);
        }
        self.settings.workspace.bind(w, k, None);
        self.histories.remove(&w);
        self.host.show(cx);
        self.sync(cx);
        self.save(cx);
        window.focus(&self.term_focus, cx);
        cx.notify();
    }

    /// A session goes where it is wanted: the focused window if empty, else
    /// the first empty one, else a new window, else the focused one.
    pub fn place(&mut self, k: Key, window: &mut Window, cx: &mut Context<Self>) {
        self.acted = true;
        if let Some((t, w)) = self.settings.workspace.bound(&k) {
            self.settings.workspace.show_tab(t);
            self.settings.workspace.focus = w;
            if self.settings.workspace.show_tag(w, &k) {
                self.histories.remove(&w);
            }
            self.sync(cx);
            self.host.show(cx);
            window.focus(&self.term_focus, cx);
            self.save(cx);
            cx.notify();
            return;
        }
        let ws = &mut self.settings.workspace;
        let w = match ws.empty_window() {
            Some(w) => w,
            None if ws.add_window() => ws.focus,
            None => ws.focus,
        };
        self.attach(k, w, window, cx);
    }

    /// Bring a session and optional immutable pane id into view. Resolve the
    /// whole request before changing the workspace, including a bad pane.
    fn reveal_session(
        &mut self,
        id: &str,
        node: Option<NodeId>,
        part: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let node = node
            .map(|n| n.to_string())
            .or_else(|| self.fleet.here_id().map(str::to_owned))
            .unwrap_or_else(|| "here".into());
        let candidates: Vec<_> = self.fleet.all.iter().filter(|s| s.node == node).collect();
        let by_id = candidates.iter().copied().find(|s| s.id == id);
        let names: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|s| s.name == id)
            .collect();
        let session = by_id.or_else(|| (names.len() == 1).then(|| names[0]));
        let Some(session) = session else {
            let why = if names.len() > 1 {
                format!("several sessions named {id} on {node}; use a session id")
            } else {
                format!("no session {id} on {node}")
            };
            self.report(&why, true, None);
            self.flush(cx);
            cx.notify();
            return;
        };
        let pane = match part {
            None => None,
            Some(part) => match part
                .strip_prefix('%')
                .unwrap_or(part)
                .parse::<u32>()
                .ok()
                .filter(|p| session.panes.iter().any(|pane| pane.id == *p))
            {
                Some(pane) => Some(pane),
                None => {
                    self.report(
                        &format!("no pane {part} in session {id} on {node}"),
                        true,
                        None,
                    );
                    self.flush(cx);
                    cx.notify();
                    return;
                }
            },
        };
        let key = Key::of(session);
        // Bindings in hidden windows and tabs remain the operator's intent.
        let bound = self
            .settings
            .workspace
            .tabs
            .iter()
            .enumerate()
            .find_map(|(tab, t)| {
                t.windows
                    .iter()
                    .enumerate()
                    .find(|(_, w)| w.sessions.iter().any(|b| b.key == key))
                    .map(|(w, _)| (tab, w))
            });
        let w = if let Some((tab, w)) = bound {
            let ws = &mut self.settings.workspace;
            ws.count = ws.count.max(tab + 1);
            if ws.tabs[tab].count <= w {
                ws.set_windows(tab, w + 1);
            }
            ws.show_tab(tab);
            ws.focus = w;
            if ws.show_tag(w, &key) {
                self.histories.remove(&w);
            }
            w
        } else {
            let w = self.settings.workspace.focus;
            self.attach(key, w, window, cx);
            w
        };
        if let Some(pane) = pane {
            self.settings.workspace.choose_pane(w, pane);
            self.histories.remove(&w);
        }
        self.sync(cx);
        self.save(cx);
        self.host.show(cx);
        window.focus(&self.term_focus, cx);
        cx.notify();
    }

    pub fn open_peek(
        &mut self,
        k: Key,
        line: Option<(u32, u32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = line.map(|(p, _)| p);
        let node = k.node.clone();
        self.peek = Some(Peek {
            key: k,
            pane,
            history: None,
            line: line.map(|(_, l)| l),
            loading: line.is_some(),
        });
        if let Some((p, _)) = line
            && let Some(id) = self.fleet.history(&node, p)
        {
            self.history_wanted.insert(id, HistoryTarget::Peek);
        }
        self.open(Surface::Peek, window, cx);
        self.sync(cx);
    }

    pub fn open_match(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self
            .fleet
            .found()
            .and_then(|(matches, _)| matches.get(i).map(|m| (*m).clone()))
        else {
            return;
        };
        let Some(s) = self.fleet.session_of_pane(&m.node, m.pane) else {
            return;
        };
        let k = Key::of(s);
        self.place(k, window, cx);
        // A drawer lying over the windows would hide the match; it steps
        // away, and Alt+H brings it back with the same results.
        if self.drawer_room(window, cx).is_none() {
            self.close(surface::FIND, cx);
            window.focus(&self.term_focus, cx);
        }
        let w = self.settings.workspace.focus;
        self.settings.workspace.choose_pane(w, m.pane);
        self.histories.remove(&w);
        if let Some(id) = self.fleet.history(&m.node, m.pane) {
            self.history_wanted.insert(
                id,
                HistoryTarget::Window {
                    index: w,
                    line: Some(m.line as usize),
                },
            );
        }
        self.sync(cx);
        cx.notify();
    }

    // ---- history ----

    pub fn open_history(&mut self, w: usize, cx: &mut Context<Self>) {
        let Some(Some(pane)) = self.window_panes().get(w).cloned() else {
            return;
        };
        if self.histories.get(&w).is_some_and(|h| h.pane == pane)
            || self
                .history_wanted
                .values()
                .any(|x| matches!(x, HistoryTarget::Window { index, .. } if *index == w))
        {
            return;
        }
        if let Some(id) = self.fleet.history(&pane.node, pane.pane) {
            self.history_wanted.insert(
                id,
                HistoryTarget::Window {
                    index: w,
                    line: None,
                },
            );
        }
        cx.notify();
    }

    fn history_arrived(
        &mut self,
        id: u32,
        pane: PaneAt,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.history_wanted.remove(&id) else {
            return;
        };
        let Some(text) = text else {
            self.report("The pane is gone; its history went with it.", true, None);
            return;
        };
        let (cols, rows) = self
            .fleet
            .screen(&pane.node, pane.pane)
            .map(|s| {
                let s = s.lock();
                (s.cols, s.rows)
            })
            .or_else(|| {
                self.fleet
                    .session_of_pane(&pane.node, pane.pane)
                    .and_then(|s| s.panes.iter().find(|p| p.id == pane.pane))
                    .map(|p| (p.cols as usize, p.rows as usize))
            })
            .unwrap_or((80, 24));
        let query = self.find_query.read(cx).text.clone();
        match target {
            HistoryTarget::Window { index: w, line } => {
                if self.window_panes().get(w).and_then(Option::as_ref) != Some(&pane) {
                    return;
                }
                let mut screen = Screen::history(&text, cols, rows, line);
                if let Some(line) = line {
                    select_line(&mut screen, line, &query);
                } else {
                    // Open one screen up from the bottom: the wheel asked to go back.
                    screen
                        .term
                        .scroll_display(alacritty_terminal::grid::Scroll::Delta(3));
                }
                self.histories.insert(
                    w,
                    History {
                        pane,
                        screen: Arc::new(Mutex::new(screen)),
                    },
                );
            }
            HistoryTarget::Peek => {
                if let Some(p) = &mut self.peek {
                    let line = p.line.map(|l| l as usize);
                    let rows = rows.max(self.peek_rows);
                    let mut screen = Screen::history(&text, cols, rows, line);
                    if let Some(l) = line {
                        select_line(&mut screen, l, &query);
                    }
                    p.history = Some(Arc::new(Mutex::new(screen)));
                    p.loading = false;
                }
            }
        }
        cx.notify();
    }
}

impl Page for Terminals {
    const ID: &'static str = crate::NAME;
    const TITLE: &'static str = "Terminals";
    const KEY: Option<char> = Some('T');
    const AREA: Option<&'static str> = Some(crate::NAME);
    const KINDS: Kinds = parvi_terminals_contract::KINDS;
    const ANSWERS: &'static [Answers] =
        &[Answers::Send, Answers::Launch, Answers::Reveal("session")];
    const SETTINGS: &'static [Setting] = crate::store::settings::DECLARED;

    /// The Settings page changed one of ours: taken, the rest as it is.
    fn settings_changed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        match self.host.setting::<Settings>(cx) {
            Ok(fresh) => self.settings.take_declared(fresh.mend()),
            Err(e) => self.report(
                &format!("The terminals' settings did not read: {}", e.message),
                true,
                None,
            ),
        }
        self.flush(cx);
        cx.notify();
    }

    fn frame(
        &mut self,
        node: &NodeId,
        kind: u8,
        body: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.frame_from(node, kind, body, window, cx);
    }

    fn link(
        &mut self,
        node: &NodeId,
        state: Option<LinkState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.link_changed(node, state, window, cx);
    }

    fn tabs(
        &mut self,
        shown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        Some(self.render_tabs(shown, window, cx))
    }

    fn shown(&mut self, shown: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = shown;
        cx.notify();
    }

    /// Send to Session with what another page hands over, aimed at the
    /// session the ask names, else at the focused window's; or a launch
    /// another page asks for.
    fn answer(&mut self, ask: Ask, _: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        match ask {
            Ask::Send { reference, target } => {
                let target = match target {
                    Some(t) => Some(Key {
                        node: t.node.to_string(),
                        user: t.user,
                        name: t.name,
                    }),
                    None => self.focused_key(),
                };
                self.handoff(Some(reference), target, window, cx);
            }
            Ask::Reveal {
                what,
                id,
                node,
                part,
                ..
            } if what == "session" => {
                self.reveal_session(&id, node, part.as_deref(), window, cx);
            }
            Ask::Launch { node, folder, name } => {
                self.launch_asked(node.as_str(), &folder, &name);
                self.flush(cx);
                cx.notify();
            }
            _ => {}
        }
    }

    fn chords(&self, _cx: &App) -> Vec<Chord> {
        self.chord_list()
    }

    /// The terminal, or the rail while rows are picked there: it holds
    /// Delete and Escape, as CHROTE's panel does once its menu closes.
    fn focus(&self, _cx: &App) -> Option<FocusHandle> {
        if !self.pick.keys.is_empty() && self.settings.sessions_open {
            Some(self.rail_focus.clone())
        } else {
            Some(self.term_focus.clone())
        }
    }
}

impl Render for Terminals {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = window.viewport_size().height - px(size::TAB_BAR + size::STATUS);
        let cell = crate::term::paint::metrics(window, self.settings.font_size)
            .cell
            .height;
        self.peek_rows = ((page * size::PEEK_SHARE - px(size::PEEK_HEAD)) / cell).max(1.) as usize;
        self.see_focused(window);
        self.render_terminals(window, cx)
    }
}

/// What Restart asks the node for, retaining a witnessed launch recipe or
/// falling back to a shell in the last observed folder.
fn restart_launch(k: &Key, saved: Option<&crate::store::settings::Restart>) -> Launch {
    Launch {
        harness: saved
            .and_then(|s| s.harness.clone())
            .unwrap_or_else(|| "shell".into()),
        folder: saved.map(|s| s.folder.clone()).unwrap_or_default(),
        name: k.name.clone(),
        flags: saved
            .filter(|s| s.harness.is_some())
            .map(|s| s.flags.clone())
            .unwrap_or_default(),
        user: k.user.clone(),
        notify: saved.is_some_and(|s| s.harness.is_some() && s.notify),
        size: None,
    }
}

/// Marks a history line as selected, so a search result is found at once.
fn select_line(s: &mut Screen, line: usize, query: &str) {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::index::{Column, Line, Point as GP, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};
    let Some((start, end)) = s.history_rows(line) else {
        return;
    };
    let history = s.term.grid().history_size();
    let cols = s.term.grid().columns();
    // A joined tmux line can span several rows. Bring the row containing the
    // query into view while selecting the whole logical line.
    let needle = query.to_lowercase();
    let target = (start..=end)
        .find(|&row| {
            let text: String = (0..cols)
                .map(|col| s.term.grid()[GP::new(Line(row as i32 - history as i32), Column(col))].c)
                .collect();
            !needle.is_empty() && text.to_lowercase().contains(&needle)
        })
        .unwrap_or(start);
    let total = history + s.rows;
    let desired = total.saturating_sub(target + 1).saturating_sub(s.rows / 2);
    let current = s.term.grid().display_offset();
    s.term
        .scroll_display(Scroll::Delta(desired as i32 - current as i32));
    let start = start as i32 - history as i32;
    let end = end as i32 - history as i32;
    let mut sel = Selection::new(
        SelectionType::Lines,
        GP::new(Line(start), Column(0)),
        Side::Left,
    );
    sel.update(
        GP::new(Line(end), Column(cols.saturating_sub(1))),
        Side::Right,
    );
    s.term.selection = Some(sel);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_starts_the_same_name_and_user_again_as_a_shell_at_home() {
        let k = Key {
            node: "n".into(),
            user: "alice".into(),
            name: "build".into(),
        };
        let l = restart_launch(&k, None);
        assert_eq!((l.name.as_str(), l.user.as_str()), ("build", "alice"));
        assert_eq!(
            (l.harness.as_str(), l.folder.as_str(), l.flags.as_str()),
            ("shell", "", "")
        );
        assert!(!l.notify);
        let saved = crate::store::settings::Restart {
            id: "$1".into(),
            folder: "/tmp/work".into(),
            harness: Some("fake".into()),
            flags: "--chosen".into(),
            notify: true,
        };
        let l = restart_launch(&k, Some(&saved));
        assert_eq!(
            (
                l.harness.as_str(),
                l.folder.as_str(),
                l.flags.as_str(),
                l.notify
            ),
            ("fake", "/tmp/work", "--chosen", true)
        );
        let fallback = crate::store::settings::Restart {
            harness: None,
            ..saved
        };
        let l = restart_launch(&k, Some(&fallback));
        assert_eq!(
            (
                l.harness.as_str(),
                l.folder.as_str(),
                l.flags.as_str(),
                l.notify
            ),
            ("shell", "/tmp/work", "", false)
        );
    }
}
