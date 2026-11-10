//! The terminal workspace, as CHROTE's (ADR-0017 decision 5): tabs, each a
//! layout of one to four windows, each window holding the sessions the
//! operator bound to it as tags, one of them on screen. A binding is intent,
//! not a cache of live sessions: it stays until the operator removes it,
//! whatever happens to the session. A session is bound in one window at a
//! time; binding it elsewhere moves it.
//!
//! Every tab keeps four windows and every workspace six tabs; the counts say
//! how many are shown. Showing fewer hides the rest with their bindings and
//! names, and showing more brings them back: a layout shrinks, it never
//! forgets. The app owns all of this; losing it loses a layout, never work.

use super::presets::Preset;
use super::sessions::{Key, find};
use parvi_terminals_contract::Session;
use serde::{Deserialize, Serialize};

pub const MAX_TABS: usize = 6;
pub const MAX_WINDOWS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    #[serde(flatten)]
    pub key: Key,
    /// The pane on screen, for sessions with more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u32>,
}

/// A window: the sessions bound to it, one tag each, and the tag on screen.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "StoredWindow")]
pub struct Window {
    pub sessions: Vec<Binding>,
    /// The tag on screen.
    pub active: usize,
}

/// A window as stored: today's, or one binding or none, as an app before
/// tags kept it.
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredWindow {
    One(Binding),
    Tags {
        #[serde(default)]
        sessions: Vec<Binding>,
        #[serde(default)]
        active: usize,
    },
    Empty(()),
}

impl From<StoredWindow> for Window {
    fn from(s: StoredWindow) -> Self {
        let (sessions, active) = match s {
            StoredWindow::One(b) => (vec![b], 0),
            StoredWindow::Tags { sessions, active } => (sessions, active),
            StoredWindow::Empty(()) => (vec![], 0),
        };
        let active = active.min(sessions.len().saturating_sub(1));
        Self { sessions, active }
    }
}

impl Window {
    /// The binding on screen.
    pub fn shown(&self) -> Option<&Binding> {
        self.sessions.get(self.active)
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    fn holds(&self, key: &Key) -> Option<usize> {
        self.sessions.iter().position(|b| &b.key == key)
    }

    /// Drops a tag; the one on screen stays, or the first takes its place.
    fn drop_tag(&mut self, at: usize) {
        self.sessions.remove(at);
        if at == self.active {
            self.active = 0;
        } else if at < self.active {
            self.active -= 1;
        }
    }
}

fn windows() -> Vec<Window> {
    vec![Window::default(); MAX_WINDOWS]
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    /// The operator's name for it; unnamed tabs read TERMINAL, TERMINAL 2…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Always four; the first `count` are shown.
    #[serde(default)]
    pub windows: Vec<Window>,
    #[serde(default)]
    pub count: usize,
}

impl Tab {
    fn new() -> Self {
        Self {
            name: None,
            windows: windows(),
            count: 1,
        }
    }

    pub fn shown(&self) -> &[Window] {
        &self.windows[..self.count]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    /// Always six; the first `count` are shown.
    pub tabs: Vec<Tab>,
    #[serde(default)]
    pub count: usize,
    pub active: usize,
    /// The focused window in the active tab.
    pub focus: usize,
    /// Layouts saved by name, at most ten (CHROTE's cap).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<Preset>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            tabs: (0..MAX_TABS).map(|_| Tab::new()).collect(),
            count: 1,
            active: 0,
            focus: 0,
            presets: vec![],
        }
    }
}

/// What a window shows.
#[derive(Clone, Debug, PartialEq)]
pub enum TileState<'a> {
    Empty,
    Live {
        session: &'a Session,
        pane: u32,
    },
    /// Bound, and the node has not said yet whether the session is there.
    Connecting(&'a str),
    /// The connection is gone; the last screen stays with Reconnect.
    Lost(&'a str),
    /// The session is gone from a list the node sent.
    Ended(&'a str),
}

/// The link's state as a tile needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Connecting,
    Open,
    Lost,
    Refused,
}

impl Workspace {
    /// The tabs on the bar.
    pub fn shown_tabs(&self) -> &[Tab] {
        &self.tabs[..self.count]
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// The windows of the active tab on screen.
    pub fn windows(&self) -> &[Window] {
        self.tab().shown()
    }

    /// The binding window `i` of the active tab shows.
    pub fn shown(&self, i: usize) -> Option<&Binding> {
        self.windows().get(i)?.shown()
    }

    /// Shows one more tab: the next hidden one, with what it held.
    pub fn add_tab(&mut self) -> bool {
        if self.count >= MAX_TABS {
            return false;
        }
        self.count += 1;
        self.active = self.count - 1;
        self.focus = 0;
        true
    }

    /// Hides a tab, with its name and bindings, behind the shown ones; the
    /// last one stays.
    pub fn close_tab(&mut self, index: usize) -> bool {
        if self.count <= 1 || index >= self.count {
            return false;
        }
        let tab = self.tabs.remove(index);
        self.tabs.insert(self.count - 1, tab);
        self.count -= 1;
        if self.active > index || self.active >= self.count {
            self.active = self.active.saturating_sub(1);
        }
        self.focus = self.focus.min(self.windows().len() - 1);
        true
    }

    pub fn show_tab(&mut self, index: usize) -> bool {
        if index >= self.count || index == self.active {
            return false;
        }
        self.active = index;
        self.focus = 0;
        true
    }

    /// Shows one more window: the next hidden one, with what it held.
    pub fn add_window(&mut self) -> bool {
        let n = self.windows().len();
        if n >= MAX_WINDOWS {
            return false;
        }
        self.tab_mut().count += 1;
        self.focus = n;
        true
    }

    /// Hides the last window if it is empty: the layout does not shrink
    /// over somebody's live terminal (Alt+Minus).
    pub fn remove_window(&mut self) -> bool {
        let w = self.windows();
        if w.len() <= 1 || !w[w.len() - 1].is_empty() {
            return false;
        }
        self.tab_mut().count -= 1;
        self.focus = self.focus.min(self.windows().len() - 1);
        true
    }

    /// Sets how many windows tab `tab` shows; the rest are hidden with
    /// their bindings (the tab menu's Windows).
    pub fn set_windows(&mut self, tab: usize, n: usize) {
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        t.count = n.clamp(1, MAX_WINDOWS);
        if tab == self.active {
            self.focus = self.focus.min(t.count - 1);
        }
    }

    pub fn focus_window(&mut self, index: isize) {
        let n = self.windows().len() as isize;
        self.focus = index.rem_euclid(n) as usize;
    }

    /// Binds a session to window `window` of tab `tab` as its tag on
    /// screen, moving it from any other window it was bound to.
    pub fn bind_in(&mut self, tab: usize, window: usize, key: Key, pane: Option<u32>) {
        if tab >= MAX_TABS || window >= MAX_WINDOWS {
            return;
        }
        self.unbind_everywhere(&key);
        let w = &mut self.tabs[tab].windows[window];
        w.sessions.push(Binding { key, pane });
        w.active = w.sessions.len() - 1;
        if tab == self.active {
            self.focus = window;
        }
    }

    /// Binds in the active tab.
    pub fn bind(&mut self, window: usize, key: Key, pane: Option<u32>) {
        self.bind_in(self.active, window, key, pane);
    }

    /// Removes one tag from window `window` of the active tab.
    pub fn unbind(&mut self, window: usize, key: &Key) {
        if let Some(w) = self.tab_mut().windows.get_mut(window)
            && let Some(at) = w.holds(key)
        {
            w.drop_tag(at);
        }
    }

    /// Removes a session from every window it is bound to, shown or hidden.
    pub fn unbind_everywhere(&mut self, key: &Key) {
        for tab in &mut self.tabs {
            for w in &mut tab.windows {
                if let Some(at) = w.holds(key) {
                    w.drop_tag(at);
                }
            }
        }
    }

    /// Puts a bound session on screen in its window.
    pub fn show_tag(&mut self, window: usize, key: &Key) -> bool {
        match self.tab_mut().windows.get_mut(window) {
            Some(w) => match w.holds(key) {
                Some(at) if at != w.active => {
                    w.active = at;
                    true
                }
                _ => false,
            },
            None => false,
        }
    }

    /// Empties every window of tab `tab`, shown or hidden.
    pub fn clear(&mut self, tab: usize) {
        if let Some(t) = self.tabs.get_mut(tab) {
            t.windows = windows();
        }
    }

    pub fn choose_pane(&mut self, window: usize, pane: u32) {
        if let Some(w) = self.tab_mut().windows.get_mut(window)
            && let Some(b) = w.sessions.get_mut(w.active)
        {
            b.pane = Some(pane);
        }
    }

    pub fn rename_tab(&mut self, index: usize, name: &str) {
        if let Some(t) = self.tabs.get_mut(index) {
            let n = name.trim();
            t.name = (!n.is_empty()).then(|| n.to_string());
        }
    }

    /// A session the node renamed keeps its tag, in every tab, under its
    /// new name.
    pub fn rename_session(&mut self, old: &Key, name: &str) {
        for b in self
            .tabs
            .iter_mut()
            .flat_map(|t| t.windows.iter_mut())
            .flat_map(|w| w.sessions.iter_mut())
        {
            if &b.key == old {
                b.key.name = name.to_string();
            }
        }
    }

    /// The first empty window of the active tab: the focused one if it is.
    pub fn empty_window(&self) -> Option<usize> {
        let w = self.windows();
        if w.get(self.focus).is_some_and(Window::is_empty) {
            return Some(self.focus);
        }
        w.iter().position(Window::is_empty)
    }

    /// Where a session is bound in a window on screen: tab and window.
    pub fn bound(&self, key: &Key) -> Option<(usize, usize)> {
        self.shown_tabs().iter().enumerate().find_map(|(t, tab)| {
            tab.shown()
                .iter()
                .position(|w| w.holds(key).is_some())
                .map(|w| (t, w))
        })
    }

    /// The binding each shown window of each shown tab has on screen, for
    /// subscriptions: a tag behind another one stays idle.
    pub fn on_screen(&self) -> impl Iterator<Item = &Binding> {
        self.shown_tabs()
            .iter()
            .flat_map(|t| t.shown().iter().filter_map(Window::shown))
    }

    /// Reads a stored workspace, mending what is out of range; `None` if it
    /// isn't one. A workspace stored before hidden tabs and windows shows
    /// what it had.
    pub fn parse(v: serde_json::Value) -> Option<Self> {
        let mut ws: Workspace = serde_json::from_value(v).ok()?;
        if ws.tabs.is_empty() {
            return None;
        }
        ws.tabs.truncate(MAX_TABS);
        if ws.count == 0 {
            ws.count = ws.tabs.len();
        }
        ws.tabs.resize_with(MAX_TABS, Tab::new);
        ws.count = ws.count.min(MAX_TABS);
        for t in &mut ws.tabs {
            mend(t);
        }
        ws.active = ws.active.min(ws.count - 1);
        ws.focus = ws.focus.min(ws.windows().len() - 1);
        super::presets::mend(&mut ws.presets);
        ws.dedupe();
        Some(ws)
    }

    /// Keeps each session in the first window that binds it.
    pub(super) fn dedupe(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for w in self.tabs.iter_mut().flat_map(|t| t.windows.iter_mut()) {
            let mut at = 0;
            while at < w.sessions.len() {
                if seen.insert(w.sessions[at].key.clone()) {
                    at += 1;
                } else {
                    w.drop_tag(at);
                }
            }
        }
    }
}

/// Four windows, one to four of them shown.
pub(super) fn mend(t: &mut Tab) {
    t.windows.truncate(MAX_WINDOWS);
    if t.count == 0 {
        t.count = t.windows.len().max(1);
    }
    t.count = t.count.min(MAX_WINDOWS);
    t.windows.resize_with(MAX_WINDOWS, Window::default);
}

/// The label a tab carries: its name, or TERMINAL, TERMINAL 2…
pub fn tab_label(tab: &Tab, index: usize) -> String {
    match &tab.name {
        Some(n) => n.to_uppercase(),
        None if index == 0 => "TERMINAL".into(),
        None => format!("TERMINAL {}", index + 1),
    }
}

/// What the node a window's session is on looks like from here: its
/// sessions, its link, and whether it has listed them on this link. `None`
/// when the app reaches no such node now.
pub type Reach<'a> = (&'a [Session], LinkState, bool);

/// What a window shows. Until the node has listed its sessions on this link,
/// a missing session isn't evidence of an ended one. A session seen to end
/// stays ended when the connection drops.
pub fn tile_state<'a>(binding: Option<&'a Binding>, node: Option<Reach<'a>>) -> TileState<'a> {
    let Some(b) = binding else {
        return TileState::Empty;
    };
    // A device the app does not reach is away; its session hasn't ended.
    let Some((sessions, link, known)) = node else {
        return TileState::Lost(&b.key.name);
    };
    let session = find(sessions, &b.key);
    let gone = session.is_none_or(|s| s.panes.is_empty());
    if known && gone {
        return TileState::Ended(&b.key.name);
    }
    if matches!(link, LinkState::Lost | LinkState::Refused) {
        return TileState::Lost(&b.key.name);
    }
    match session {
        Some(s) if link == LinkState::Open && known => {
            let pane = s
                .panes
                .iter()
                .find(|p| Some(p.id) == b.pane)
                .unwrap_or(&s.panes[0]);
            TileState::Live {
                session: s,
                pane: pane.id,
            }
        }
        _ => TileState::Connecting(&b.key.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_terminals_contract::Pane;

    fn key(name: &str) -> Key {
        Key {
            node: "n".into(),
            user: String::new(),
            name: name.into(),
        }
    }

    fn tags(w: &Window) -> Vec<&str> {
        w.sessions.iter().map(|b| b.key.name.as_str()).collect()
    }

    fn session(name: &str, panes: &[u32]) -> Session {
        Session {
            node: "n".into(),
            id: "$1".into(),
            name: name.into(),
            panes: panes
                .iter()
                .map(|&id| Pane {
                    id,
                    cols: 80,
                    rows: 24,
                    waiting: None,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_window_holds_several_tags_and_shows_the_newest() {
        let mut ws = Workspace::default();
        ws.bind(0, key("a"), None);
        ws.bind(0, key("b"), None);
        assert_eq!(tags(&ws.windows()[0]), ["a", "b"]);
        assert_eq!(ws.shown(0).unwrap().key, key("b"));
        assert!(ws.show_tag(0, &key("a")));
        assert!(!ws.show_tag(0, &key("a")), "already on screen");
        assert_eq!(ws.shown(0).unwrap().key, key("a"));
        ws.unbind(0, &key("a"));
        assert_eq!(
            ws.shown(0).unwrap().key,
            key("b"),
            "the first tag left takes the screen"
        );
        ws.bind(0, key("c"), None);
        ws.unbind(0, &key("b"));
        assert_eq!(tags(&ws.windows()[0]), ["c"]);
        assert_eq!(ws.shown(0).unwrap().key, key("c"));
    }

    #[test]
    fn binding_a_session_elsewhere_moves_it() {
        let mut ws = Workspace::default();
        ws.add_window();
        ws.bind(0, key("a"), None);
        ws.bind(0, key("b"), None);
        ws.show_tag(0, &key("a"));
        ws.bind(1, key("a"), None);
        assert_eq!(tags(&ws.windows()[0]), ["b"]);
        assert_eq!(ws.shown(0).unwrap().key, key("b"));
        assert_eq!(tags(&ws.windows()[1]), ["a"]);
        assert_eq!(ws.focus, 1);
        ws.add_tab();
        ws.bind(0, key("a"), None);
        assert!(ws.tabs[0].windows[1].is_empty(), "across tabs too");
        assert_eq!(ws.bound(&key("a")), Some((1, 0)));
    }

    #[test]
    fn fewer_windows_hide_the_rest_with_their_bindings() {
        let mut ws = Workspace::default();
        for _ in 0..5 {
            ws.add_window();
        }
        assert_eq!(ws.windows().len(), 4);
        ws.bind(3, key("a"), None);
        assert!(
            !ws.remove_window(),
            "the layout does not shrink over a bound window"
        );
        ws.set_windows(0, 2);
        assert_eq!(ws.windows().len(), 2);
        assert_eq!(ws.focus, 1);
        assert_eq!(ws.bound(&key("a")), None, "hidden");
        assert_eq!(ws.on_screen().count(), 0);
        ws.add_window();
        ws.add_window();
        assert_eq!(ws.shown(3).unwrap().key, key("a"), "and back");
        ws.unbind(3, &key("a"));
        assert!(ws.remove_window());
        assert_eq!(ws.windows().len(), 3);
        assert_eq!(ws.focus, 2);
        ws.set_windows(0, 9);
        assert_eq!(ws.windows().len(), 4);
    }

    #[test]
    fn a_closed_tab_is_hidden_with_its_name_and_bindings() {
        let mut ws = Workspace::default();
        while ws.add_tab() {}
        assert_eq!(ws.shown_tabs().len(), MAX_TABS);
        assert_eq!(tab_label(&ws.tabs[2], 2), "TERMINAL 3");
        ws.show_tab(1);
        ws.rename_tab(1, "ops");
        ws.bind(0, key("a"), None);
        assert!(ws.close_tab(1));
        assert_eq!(ws.shown_tabs().len(), 5);
        assert_eq!(ws.active, 1, "the next tab takes its place");
        assert_eq!(ws.bound(&key("a")), None);
        assert!(ws.add_tab());
        assert_eq!(ws.active, 5);
        assert_eq!(ws.tab().name.as_deref(), Some("ops"));
        assert_eq!(ws.shown(0).unwrap().key, key("a"));
        while ws.close_tab(0) {}
        assert_eq!(ws.shown_tabs().len(), 1);
        assert_eq!(ws.active, 0);
        assert!(!ws.close_tab(0), "the last tab stays");
    }

    #[test]
    fn only_the_tag_on_screen_of_a_shown_window_is_followed() {
        let mut ws = Workspace::default();
        ws.add_window();
        ws.bind(0, key("a"), None);
        ws.bind(0, key("b"), None);
        ws.bind(1, key("c"), None);
        ws.add_tab();
        ws.bind(0, key("d"), None);
        ws.set_windows(1, 1);
        ws.add_window();
        ws.bind(1, key("e"), None);
        ws.set_windows(1, 1);
        let on: Vec<&str> = ws.on_screen().map(|b| b.key.name.as_str()).collect();
        assert_eq!(on, ["b", "c", "d"]);
    }

    #[test]
    fn a_renamed_session_keeps_its_tags_in_every_tab() {
        let mut ws = Workspace::default();
        ws.add_window();
        ws.bind(0, key("a"), Some(7));
        ws.bind(1, key("b"), None);
        let theirs = Key {
            user: "alice".into(),
            ..key("a")
        };
        ws.bind(1, theirs.clone(), None);
        ws.rename_session(&key("a"), "z");
        assert_eq!(ws.bound(&key("a")), None);
        assert_eq!(ws.shown(0).unwrap().key, key("z"));
        assert_eq!(ws.shown(0).unwrap().pane, Some(7));
        assert_eq!(tags(&ws.windows()[1]), ["b", "a"]);
        assert_eq!(
            ws.shown(1).unwrap().key,
            theirs,
            "another user's session of that name stays"
        );
    }

    #[test]
    fn clearing_a_tab_empties_its_hidden_windows_too() {
        let mut ws = Workspace::default();
        ws.set_windows(0, 3);
        ws.bind(2, key("a"), None);
        ws.set_windows(0, 1);
        ws.clear(0);
        ws.set_windows(0, 3);
        assert!(ws.windows().iter().all(Window::is_empty));
    }

    #[test]
    fn focus_wraps_both_ways() {
        let mut ws = Workspace::default();
        ws.set_windows(0, 3);
        ws.focus_window(3);
        assert_eq!(ws.focus, 0);
        ws.focus_window(-1);
        assert_eq!(ws.focus, 2);
    }

    #[test]
    fn a_tile_tells_ended_from_lost_from_connecting() {
        let live = [session("a", &[4, 5])];
        let b = Binding {
            key: key("a"),
            pane: Some(5),
        };
        assert_eq!(
            tile_state(Some(&b), Some((&live, LinkState::Open, true))),
            TileState::Live {
                session: &live[0],
                pane: 5
            }
        );
        assert_eq!(
            tile_state(Some(&b), Some((&[], LinkState::Open, true))),
            TileState::Ended("a")
        );
        assert_eq!(
            tile_state(Some(&b), Some((&live, LinkState::Lost, true))),
            TileState::Lost("a")
        );
        assert_eq!(
            tile_state(Some(&b), Some((&[], LinkState::Connecting, false))),
            TileState::Connecting("a")
        );
        assert_eq!(
            tile_state(None, Some((&live, LinkState::Open, true))),
            TileState::Empty
        );
        let other = Binding {
            key: key("a"),
            pane: Some(9),
        };
        assert_eq!(
            tile_state(Some(&other), Some((&live, LinkState::Open, true))),
            TileState::Live {
                session: &live[0],
                pane: 4
            }
        );
        assert_eq!(
            tile_state(Some(&b), None),
            TileState::Lost("a"),
            "another device's session waits for that device; it hasn't ended"
        );
    }

    #[test]
    fn a_stored_workspace_is_mended() {
        let v = serde_json::json!({"tabs":[{"windows":[]},{"name":"ops","windows":[{"node":"n","user":"","name":"a"},null,null,null,null]}],"active":7,"focus":9});
        let ws = Workspace::parse(v).unwrap();
        assert_eq!(ws.shown_tabs().len(), 2);
        assert_eq!(ws.tabs.len(), MAX_TABS);
        assert_eq!(ws.tabs[0].windows.len(), MAX_WINDOWS);
        assert_eq!(ws.tabs[0].count, 1);
        assert_eq!(ws.tabs[1].count, 4);
        assert_eq!((ws.active, ws.focus), (1, 3));
        assert_eq!(tab_label(&ws.tabs[1], 1), "OPS");
        assert_eq!(ws.shown(0).unwrap().key, key("a"));
        assert!(Workspace::parse(serde_json::json!({"tabs":[]})).is_none());

        let mut ws = Workspace::default();
        ws.add_window();
        ws.bind(0, key("a"), None);
        ws.bind(0, key("b"), None);
        ws.show_tag(0, &key("a"));
        let back = Workspace::parse(serde_json::to_value(&ws).unwrap()).unwrap();
        assert_eq!(back, ws);

        let twice = serde_json::json!({"tabs":[{"windows":[
            {"sessions":[{"node":"n","name":"a"},{"node":"n","name":"b"}],"active":5},
            {"sessions":[{"node":"n","name":"a"}]}
        ],"count":2}],"active":0,"focus":0});
        let ws = Workspace::parse(twice).unwrap();
        assert_eq!(tags(&ws.windows()[0]), ["a", "b"]);
        assert_eq!(ws.windows()[0].active, 1);
        assert!(
            ws.windows()[1].is_empty(),
            "a session is bound in one window"
        );
    }
}
