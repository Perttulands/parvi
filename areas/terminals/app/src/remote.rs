//! The page's side of one node: what the node said last (sessions,
//! launcher), the pane screens fed on the network thread, and the requests
//! in flight. It knows the terminals wire and nothing of views; it turns
//! frames into state and notices, and the page decides what to show. What it
//! sends waits in `out` until the page hands it to the node.

use crate::store::sessions;
use crate::store::workspace::LinkState;
use crate::term::screen::Screen;
use parking_lot::Mutex;
use parvi_terminals_contract::{
    Down, Launch, Launcher, MAX_SEND, Outcome, Seat, Session, Up, encode_up,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Requests are numbered across the app, so a notice names one request.
static REQUESTS: AtomicU32 = AtomicU32::new(0);

fn next_request() -> u32 {
    REQUESTS.fetch_add(1, Ordering::Relaxed) + 1
}

pub type SharedScreen = Arc<Mutex<Screen>>;
pub type Screens = Arc<Mutex<HashMap<u32, SharedScreen>>>;

/// What happened that the operator may need to hear about.
#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    /// Sessions that started waiting for an answer.
    Waiting(Vec<String>),
    /// Sessions whose agent just reported, as listed now.
    Reported(Vec<Session>),
    Launched {
        id: u32,
        ok: bool,
        text: String,
    },
    Deleted {
        id: u32,
        ok: bool,
        text: String,
    },
    Renamed {
        id: u32,
        ok: bool,
        text: String,
    },
    /// The node set tmux's mouse mode; `text` says on how many servers.
    Moused {
        id: u32,
        ok: bool,
        text: String,
    },
    Delivered {
        id: u32,
        outcome: Outcome,
    },
    History {
        id: u32,
        pane: u32,
        text: Option<String>,
    },
    ClaimRefused(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub node: String,
    pub pane: u32,
    pub line: u32,
    pub context: [String; 3],
}

#[derive(Default)]
pub struct Search {
    pub id: u32,
    pub matches: Vec<Match>,
    /// matches, panes read, truncated: set when the search is over.
    pub done: Option<(u32, u32, bool)>,
}

pub struct Thumb {
    pub text: String,
}

enum Request {
    Launch,
    Delete,
    Rename,
    Mouse,
    Send,
    History,
}

pub struct Remote {
    /// The node, by its id.
    pub node: String,
    /// The link to it, as the app last said.
    pub state: LinkState,
    /// Frames for the node, in order, until the page sends them.
    pub out: Vec<Vec<u8>>,
    pub sessions: Vec<Session>,
    /// This connection has sent a session list.
    pub known: bool,
    pub launcher: Option<Launcher>,
    screens: Screens,
    /// Last watched panes of ended sessions, retained only while bound.
    ended_screens: HashMap<(sessions::Key, u32), SharedScreen>,
    subscribed: HashSet<u32>,
    /// The grid each pane's seat was last claimed at, by this app.
    claimed: HashMap<u32, (u16, u16)>,
    /// Panes whose seat the operator gave back: not claimed on arrival
    /// again until claimed by hand (#161).
    given_back: HashSet<u32>,
    pending: HashMap<u32, Request>,
    pub thumbs: HashMap<u32, Thumb>,
    pub search: Option<Search>,
}

impl Remote {
    pub fn new(node: &str, screens: Screens) -> Self {
        Self {
            node: node.to_string(),
            state: LinkState::Connecting,
            out: vec![],
            sessions: vec![],
            known: false,
            launcher: None,
            screens,
            ended_screens: HashMap::new(),
            subscribed: HashSet::new(),
            claimed: HashMap::new(),
            given_back: HashSet::new(),
            pending: HashMap::new(),
            thumbs: HashMap::new(),
            search: None,
        }
    }

    fn send(&mut self, up: Up) {
        if self.open() {
            self.out.push(encode_up(&up));
        }
    }

    fn request(&mut self, r: Request) -> u32 {
        let id = next_request();
        self.pending.insert(id, r);
        id
    }

    fn open(&self) -> bool {
        self.state == LinkState::Open
    }

    /// The link opened: what this app had on the last one is gone.
    pub fn opened(&mut self) {
        self.state = LinkState::Open;
        self.known = false;
        self.subscribed.clear();
        self.claimed.clear();
    }

    /// The link went. Pending requests will not be answered on a new
    /// socket; a send may have gone in before it went: unknown.
    pub fn closed(&mut self) -> Vec<Notice> {
        let unknown: Vec<Notice> = self
            .pending
            .drain()
            .filter(|(_, r)| matches!(r, Request::Send))
            .map(|(id, _)| Notice::Delivered {
                id,
                outcome: Outcome::Unknown,
            })
            .collect();
        if let Some(s) = &mut self.search
            && s.done.is_none()
        {
            s.done = Some((s.matches.len() as u32, 0, true));
        }
        self.out.clear();
        self.subscribed.clear();
        self.claimed.clear();
        unknown
    }

    pub fn frame(&mut self, down: Down) -> Vec<Notice> {
        let node = self.node.clone();
        match down {
            Down::Sessions(mut list) => {
                for s in &mut list {
                    s.node = node.clone();
                }
                // Keep the last watched frame if the list arrives before
                // PaneGone. A PaneGone already captured it before a stale
                // list could cause a fresh, empty subscription.
                for old in &self.sessions {
                    if !list
                        .iter()
                        .any(|s| sessions::is(s, &sessions::Key::of(old)))
                    {
                        for pane in &old.panes {
                            if let Some(screen) = self.screen(pane.id) {
                                self.ended_screens
                                    .entry((sessions::Key::of(old), pane.id))
                                    .or_insert(screen);
                            }
                        }
                    }
                }
                // A new incarnation under the same name must not inherit an
                // earlier incarnation's terminal.
                for s in &list {
                    if !self
                        .sessions
                        .iter()
                        .any(|old| sessions::is(old, &sessions::Key::of(s)) && old.id == s.id)
                    {
                        self.ended_screens
                            .retain(|(key, _), _| key != &sessions::Key::of(s));
                    } else {
                        // A live session can shed panes indefinitely; frames
                        // of its old panes are no longer its ended frame.
                        self.ended_screens.retain(|(key, pane), _| {
                            key != &sessions::Key::of(s) || s.panes.iter().any(|p| p.id == *pane)
                        });
                    }
                }
                let started: Vec<String> = if self.known {
                    sessions::started_waiting(&self.sessions, &list)
                        .iter()
                        .map(|s| s.name.clone())
                        .collect()
                } else {
                    vec![]
                };
                // The first list after connecting is history, not news.
                let reported: Vec<Session> = if self.known {
                    sessions::reported(&self.sessions, &list)
                        .into_iter()
                        .cloned()
                        .collect()
                } else {
                    vec![]
                };
                // A seat no longer ours is not ours to resize.
                for s in &list {
                    if s.seat != Some(Seat::Mine) {
                        for p in &s.panes {
                            self.claimed.remove(&p.id);
                        }
                    }
                }
                self.sessions = list;
                self.known = true;
                let mut notices = vec![];
                if !started.is_empty() {
                    notices.push(Notice::Waiting(started));
                }
                if !reported.is_empty() {
                    notices.push(Notice::Reported(reported));
                }
                notices
            }
            Down::Launcher(l) => {
                self.launcher = Some(l);
                vec![]
            }
            Down::PaneGone(p) => {
                if let Some(screen) = self.screen(p)
                    && let Some(session) = self
                        .sessions
                        .iter()
                        .find(|s| s.panes.iter().any(|pane| pane.id == p))
                {
                    self.ended_screens
                        .entry((sessions::Key::of(session), p))
                        .or_insert(screen);
                }
                self.subscribed.remove(&p);
                self.claimed.remove(&p);
                vec![]
            }
            Down::Thumbnail { pane, text, .. } => {
                self.thumbs.insert(pane, Thumb { text });
                vec![]
            }
            Down::Done { id, ok, text } => match self.pending.remove(&id) {
                Some(Request::Launch) => vec![Notice::Launched { id, ok, text }],
                Some(Request::Delete) => vec![Notice::Deleted { id, ok, text }],
                Some(Request::Rename) => vec![Notice::Renamed { id, ok, text }],
                Some(Request::Mouse) => vec![Notice::Moused { id, ok, text }],
                _ => vec![],
            },
            Down::Delivered { id, outcome } => match self.pending.remove(&id) {
                Some(Request::Send) => vec![Notice::Delivered { id, outcome }],
                _ => vec![],
            },
            Down::History { id, pane, text } => match self.pending.remove(&id) {
                Some(Request::History) => vec![Notice::History { id, pane, text }],
                _ => vec![],
            },
            Down::Match {
                id,
                pane,
                line,
                context,
            } => {
                if let Some(s) = &mut self.search
                    && s.id == id
                {
                    s.matches.push(Match {
                        node: node.clone(),
                        pane,
                        line,
                        context,
                    });
                }
                vec![]
            }
            Down::Searched {
                id,
                matches,
                panes,
                truncated,
            } => {
                if let Some(s) = &mut self.search
                    && s.id == id
                {
                    s.done = Some((matches, panes, truncated));
                }
                vec![]
            }
            Down::ClaimRefused { pane, reason } => {
                self.claimed.remove(&pane);
                vec![Notice::ClaimRefused(reason)]
            }
            Down::Screen { .. } | Down::Output { .. } => vec![],
        }
    }

    pub fn screen(&self, pane: u32) -> Option<SharedScreen> {
        self.screens.lock().get(&pane).cloned()
    }

    pub fn ended_screen(&self, key: &sessions::Key, pane: Option<u32>) -> Option<SharedScreen> {
        pane.and_then(|p| self.ended_screens.get(&(key.clone(), p)))
            .or_else(|| {
                self.ended_screens
                    .iter()
                    .find_map(|((k, _), screen)| (k == key).then_some(screen))
            })
            .cloned()
    }

    pub fn retain_ended_screens(&mut self, bindings: &HashSet<sessions::Key>) {
        self.ended_screens
            .retain(|(key, _), _| bindings.contains(key));
    }

    /// Subscribes exactly these panes: new ones get a fresh screen, others
    /// are unsubscribed and their screens dropped.
    pub fn watch(&mut self, panes: &HashSet<u32>) {
        if !self.open() {
            return;
        }
        let gone: Vec<u32> = self.subscribed.difference(panes).copied().collect();
        for p in gone {
            self.subscribed.remove(&p);
            self.screens.lock().remove(&p);
            if self.claimed.remove(&p).is_some() {
                self.send(Up::Release(p));
            }
            self.send(Up::Unsubscribe(p));
        }
        for &p in panes {
            if self.subscribed.insert(p) {
                let (cols, rows) = self.pane_grid(p).unwrap_or((80, 24));
                self.screens.lock().insert(
                    p,
                    Arc::new(Mutex::new(Screen::new(cols as usize, rows as usize))),
                );
                self.send(Up::Subscribe(p));
            }
        }
    }

    fn pane_grid(&self, pane: u32) -> Option<(u16, u16)> {
        self.sessions
            .iter()
            .flat_map(|s| &s.panes)
            .find(|p| p.id == pane)
            .map(|p| (p.cols, p.rows))
    }

    pub fn session_of_pane(&self, pane: u32) -> Option<&Session> {
        self.sessions
            .iter()
            .find(|s| s.panes.iter().any(|p| p.id == pane))
    }

    pub fn input(&mut self, pane: u32, bytes: &[u8]) {
        self.send(Up::Input(pane, bytes.to_vec()));
    }

    /// Whether this app may size the session without taking anything from
    /// anyone: nobody holds the seat, no other client is attached, and the
    /// window's size isn't pinned.
    pub fn free_to_size(s: &Session) -> bool {
        s.seat.is_none() && s.foreign.unwrap_or(0) == 0 && s.pinned != Some(true)
    }

    /// Whether the window showing a pane of `s` sizes it: while this app
    /// holds the seat, and on arrival when the seat is free (CHROTE's
    /// claim on arrival), unless the operator gave it back here.
    pub fn sizes_here(s: &Session, held: bool, given_back: bool) -> bool {
        !given_back && (held || Remote::free_to_size(s))
    }

    /// Claims the seat at this grid, or follows the grid while holding it.
    /// Sends nothing if the seat is ours at this grid already.
    pub fn claim(&mut self, pane: u32, cols: u16, rows: u16) {
        self.given_back.remove(&pane);
        if !self.open() || self.claimed.get(&pane) == Some(&(cols, rows)) {
            return;
        }
        self.claimed.insert(pane, (cols, rows));
        self.send(Up::Claim(pane, cols, rows));
    }

    /// Claims even at the grid last claimed, as the operator's Claim does.
    pub fn claim_anew(&mut self, pane: u32, cols: u16, rows: u16) {
        self.claimed.remove(&pane);
        self.claim(pane, cols, rows);
    }

    /// Whether this app holds the pane's seat: it claimed it, or the node
    /// says so (a list sent before our claim arrived may have said not).
    pub fn holds(&self, pane: u32) -> bool {
        self.claimed.contains_key(&pane)
            || self
                .session_of_pane(pane)
                .is_some_and(|s| s.seat == Some(Seat::Mine))
    }

    pub fn release(&mut self, pane: u32) {
        let held = self.holds(pane);
        self.claimed.remove(&pane);
        if held {
            self.send(Up::Release(pane));
        }
    }

    /// The operator's Release: the seat goes back, and this app does not
    /// take it again on arrival until claimed by hand.
    pub fn give_back(&mut self, pane: u32) {
        self.release(pane);
        self.given_back.insert(pane);
    }

    pub fn given_back(&self, pane: u32) -> bool {
        self.given_back.contains(&pane)
    }

    /// Sends text as one paste, Enter after if asked; with `ready`, once the
    /// just-launched pane is ready for it (#242). `None` if it is too long or
    /// there is no connection.
    pub fn paste_to(
        &mut self,
        pane: u32,
        text: &str,
        enter: bool,
        ready: bool,
    ) -> Result<u32, String> {
        if text.len() > MAX_SEND {
            return Err(format!(
                "At most {} KiB can be sent at once.",
                MAX_SEND / 1024
            ));
        }
        if !self.open() {
            return Err("Not connected.".into());
        }
        let id = self.request(Request::Send);
        self.send(Up::Send {
            pane,
            id,
            enter,
            ready,
            text: text.to_string(),
        });
        Ok(id)
    }

    pub fn launch(&mut self, launch: Launch) -> Option<u32> {
        if !self.open() {
            return None;
        }
        let id = self.request(Request::Launch);
        self.send(Up::Launch { id, launch });
        Some(id)
    }

    /// Tells the node the operator looked at this session while it showed
    /// the event heard at `at`.
    pub fn seen(&mut self, session_id: &str, at: u64) {
        self.send(Up::Seen {
            session: session_id.to_string(),
            event: at,
        });
    }

    pub fn delete(&mut self, session_id: &str) -> Option<u32> {
        if !self.open() {
            return None;
        }
        let id = self.request(Request::Delete);
        self.send(Up::Delete {
            id,
            session: session_id.to_string(),
        });
        Some(id)
    }

    pub fn rename(&mut self, session_id: &str, name: &str) -> Option<u32> {
        if !self.open() {
            return None;
        }
        let id = self.request(Request::Rename);
        self.send(Up::Rename {
            id,
            session: session_id.to_string(),
            name: name.to_string(),
        });
        Some(id)
    }

    pub fn mouse(&mut self, on: bool) -> Option<u32> {
        if !self.open() {
            return None;
        }
        let id = self.request(Request::Mouse);
        self.send(Up::Mouse { id, on });
        Some(id)
    }

    pub fn history(&mut self, pane: u32) -> Option<u32> {
        if !self.open() {
            return None;
        }
        let id = self.request(Request::History);
        self.send(Up::History { pane, id });
        Some(id)
    }

    /// The overview stopped: its thumbnails go.
    pub fn overview_off(&mut self) {
        self.thumbs.clear();
    }

    /// Starts a search (an empty query stops the one running).
    pub fn search(&mut self, query: &str) {
        let id = next_request();
        self.send(Up::Search {
            id,
            query: query.trim().to_string(),
        });
        self.search = (!query.trim().is_empty()).then(|| Search {
            id,
            ..Default::default()
        });
    }
}

/// The network thread's half: screen and output frames go straight into
/// the pane's terminal. Wakes the UI only for the first change since it
/// last painted.
pub fn feed(screens: &Screens, d: &Down) -> bool {
    let (pane, bytes, grid) = match d {
        Down::Screen {
            pane,
            cols,
            rows,
            bytes,
        } => (*pane, bytes, Some((*cols as usize, *rows as usize))),
        Down::Output { pane, bytes } => (*pane, bytes, None),
        _ => return false,
    };
    let Some(screen) = screens.lock().get(&pane).cloned() else {
        return false;
    };
    let mut s = screen.lock();
    if let Some((cols, rows)) = grid {
        s.reset(cols, rows);
    }
    s.feed(bytes);
    let woke = !s.dirty;
    s.dirty = true;
    woke
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ended_session_keeps_its_last_watched_frame_until_unbound_or_restarted() {
        let mut client = Remote::new("node", Screens::default());
        client.opened();
        let session = Session {
            node: "node".into(),
            id: "$1".into(),
            name: "agent".into(),
            panes: vec![parvi_terminals_contract::Pane {
                id: 7,
                cols: 80,
                rows: 24,
                ..Default::default()
            }],
            ..Default::default()
        };
        let key = sessions::Key::of(&session);
        client.sessions.push(session.clone());
        let last = Arc::new(Mutex::new(Screen::new(80, 24)));
        client.screens.lock().insert(7, last.clone());
        client.subscribed.insert(7);

        client.frame(Down::PaneGone(7));
        // A stale session list can make watch subscribe again before the
        // session disappears, replacing the live screen with an empty one.
        client.watch(&HashSet::from([7]));
        client.frame(Down::PaneGone(7));
        client.frame(Down::Sessions(vec![]));
        assert!(Arc::ptr_eq(
            &client.ended_screen(&key, Some(7)).unwrap(),
            &last
        ));
        client.retain_ended_screens(&HashSet::from([key.clone()]));
        assert!(client.ended_screen(&key, None).is_some());

        let restarted = Session {
            id: "$2".into(),
            ..session.clone()
        };
        client.frame(Down::Sessions(vec![restarted]));
        assert!(client.ended_screen(&key, None).is_none());

        // Session lists can also arrive before PaneGone.
        let second = Arc::new(Mutex::new(Screen::new(80, 24)));
        client.screens.lock().insert(7, second.clone());
        client.frame(Down::Sessions(vec![]));
        assert!(Arc::ptr_eq(
            &client.ended_screen(&key, Some(7)).unwrap(),
            &second
        ));
        client.retain_ended_screens(&HashSet::new());
        assert!(client.ended_screen(&key, None).is_none());

        client.frame(Down::Sessions(vec![session.clone()]));
        client.frame(Down::PaneGone(7));
        let changed_pane = Session {
            panes: vec![parvi_terminals_contract::Pane {
                id: 8,
                cols: 80,
                rows: 24,
                ..Default::default()
            }],
            ..session
        };
        client.frame(Down::Sessions(vec![changed_pane]));
        assert!(client.ended_screen(&key, None).is_none());
    }

    #[test]
    fn frames_feed_only_watched_panes_and_wake_once() {
        let screens: Screens = Default::default();
        screens
            .lock()
            .insert(3, Arc::new(Mutex::new(Screen::new(10, 2))));
        let screen = Down::Screen {
            pane: 3,
            cols: 20,
            rows: 5,
            bytes: b"hi".to_vec(),
        };
        assert!(feed(&screens, &screen));
        assert!(
            !feed(
                &screens,
                &Down::Output {
                    pane: 3,
                    bytes: b"!".to_vec()
                }
            ),
            "one wake until painted"
        );
        assert!(!feed(
            &screens,
            &Down::Output {
                pane: 4,
                bytes: b"x".to_vec()
            }
        ));
        let s = screens.lock().get(&3).cloned().unwrap();
        let s = s.lock();
        assert_eq!((s.cols, s.rows), (20, 5));
    }

    #[test]
    fn a_window_sizes_a_free_session_on_arrival_until_given_back() {
        let mut s = Session::default();
        assert!(Remote::sizes_here(&s, false, false), "claimed on arrival");
        assert!(!Remote::sizes_here(&s, false, true), "given back here");
        s.seat = Some(Seat::Other);
        assert!(!Remote::sizes_here(&s, false, false), "held elsewhere");
        s.seat = Some(Seat::Mine);
        assert!(Remote::sizes_here(&s, true, false), "held here");
        assert!(!Remote::sizes_here(&s, true, true));
    }

    #[test]
    fn only_a_free_session_is_sized_on_arrival() {
        let mut s = Session::default();
        assert!(Remote::free_to_size(&s));
        s.foreign = Some(1);
        assert!(!Remote::free_to_size(&s));
        s.foreign = Some(0);
        s.seat = Some(Seat::Other);
        assert!(!Remote::free_to_size(&s));
        s.seat = None;
        s.pinned = Some(true);
        assert!(!Remote::free_to_size(&s));
    }
}
