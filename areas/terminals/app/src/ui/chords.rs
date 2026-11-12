//! The page's chords, CHROTE's terminal chords: the tabs, the windows, the
//! launcher, the rail, Send, Overview and the scrollback search. They are
//! live from every page and bring the terminals forward where they act.

use super::page::Terminals;
use super::surface::{self, Surface};
use crate::store::sessions::{self, Key};
use gpui::{Context, WeakEntity, Window};
use parvi_app_kit::chords::Chord;
use parvi_app_kit::widgets::field;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Tab(usize),
    ClaimAll,
    NextWindow,
    PreviousWindow,
    AddWindow,
    RemoveWindow,
    SearchSessions,
    Launcher,
    Sessions,
    Send,
    Overview,
    Find,
    Reported,
}

/// Every chord acting on `page`, its `tabs` first, as CHROTE lists them.
fn chords(page: &WeakEntity<Terminals>, tabs: usize) -> Vec<Chord> {
    let chord = |leader: &'static str, label: &str, action: Action| {
        let page = page.clone();
        Chord::new(leader, label, move |window, cx| {
            let _ = page.update(cx, |p, cx| p.run(action, window, cx));
        })
        .everywhere()
    };
    let mut all: Vec<Chord> = TAB_KEYS
        .iter()
        .enumerate()
        .map(|(i, key)| {
            let label = if i < tabs {
                format!("Terminal tab {}", i + 1)
            } else {
                format!("Terminal tab {} (not open)", i + 1)
            };
            chord(key, &label, Action::Tab(i)).alt(key, None)
        })
        .collect();
    all.extend([
        chord("c", "Claim all sessions", Action::ClaimAll).alt("c", Some(false)),
        chord("w", "Next window", Action::NextWindow).alt("w", Some(false)),
        chord("W", "Previous window", Action::PreviousWindow).alt("w", Some(true)),
        chord("=", "Add a window", Action::AddWindow)
            .alt("+", None)
            .or_layout(&["="]),
        chord("-", "Remove the last empty window", Action::RemoveWindow).alt("-", None),
        chord("/", "Session search", Action::SearchSessions),
        chord("n", "Launcher in the focused window", Action::Launcher).alt("n", Some(false)),
        chord("tab", "Sessions panel", Action::Sessions).alt("a", Some(false)),
        chord("s", "Send from the focused session", Action::Send).alt("s", Some(false)),
        // Parvi's own, on letters CHROTE leaves free: V for view, H for
        // history (Alt+F is the shell's forward-word).
        chord("v", "Overview of every session", Action::Overview).alt("v", Some(false)),
        chord("h", "Search every scrollback", Action::Find).alt("h", Some(false)),
        // J for jump: answering a toast without finding its row.
        chord("j", "Go to the session that reported", Action::Reported).alt("j", Some(false)),
    ]);
    all
}

/// The keys of the tabs, Alt+1 to Alt+6.
const TAB_KEYS: [&str; crate::store::workspace::MAX_TABS] = ["1", "2", "3", "4", "5", "6"];

impl Terminals {
    /// Every chord of the page.
    pub fn chord_list(&self) -> Vec<Chord> {
        chords(&self.me, self.settings.workspace.shown_tabs().len())
    }

    pub fn run(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let ws = &mut self.settings.workspace;
        match action {
            Action::Tab(i) => {
                if i >= ws.count {
                    return;
                }
                if ws.show_tab(i) {
                    self.histories.clear();
                }
                self.host.show(cx);
            }
            Action::NextWindow => ws.focus_window(ws.focus as isize + 1),
            Action::PreviousWindow => ws.focus_window(ws.focus as isize - 1),
            Action::AddWindow => {
                if !ws.add_window() {
                    self.report("Four windows is the most a tab holds.", false, None);
                }
            }
            Action::RemoveWindow => {
                if !ws.remove_window() {
                    self.report("Only an empty last window can be removed.", false, None);
                }
            }
            Action::Launcher => {
                self.host.show(cx);
                // The launcher starts where the focused window's session runs.
                let near = ws.shown(ws.focus).map(|b| b.key.node.clone());
                let opened = match ws.empty_window() {
                    Some(w) => {
                        ws.focus = w;
                        Some(w)
                    }
                    None if ws.add_window() && ws.windows()[ws.focus].is_empty() => Some(ws.focus),
                    None => None,
                };
                match opened {
                    Some(w) => {
                        let l = self.launchers[w].clone();
                        l.update(cx, |l, cx| {
                            if let Some(node) = &near {
                                l.point_at(node, cx);
                            }
                            l.focus_first(window, cx);
                        });
                    }
                    None => self.report(
                        "Every window holds a session; remove one to launch here.",
                        false,
                        None,
                    ),
                }
                self.save(cx);
                cx.notify();
                return;
            }
            Action::Sessions => self.settings.sessions_open = !self.settings.sessions_open,
            Action::SearchSessions => {
                self.settings.sessions_open = true;
                self.host.show(cx);
                self.rail_search
                    .update(cx, |f, cx| f.set(&f.text.clone(), true, cx));
                field::focus(&self.rail_search, window, cx);
                self.save(cx);
                cx.notify();
                return;
            }
            Action::Send => {
                if let Some(source) = self.focused_key() {
                    self.send_from(source, window, cx);
                } else {
                    self.handoff(None, None, window, cx);
                }
                return;
            }
            Action::ClaimAll => {
                self.act(super::page::Act::ClaimAll, window, cx);
                return;
            }
            Action::Overview => {
                if self.is_open(surface::OVERVIEW) {
                    self.close(surface::OVERVIEW, cx);
                } else {
                    self.open(Surface::Overview, window, cx);
                }
                return;
            }
            Action::Reported => {
                match sessions::newest_news(&self.fleet.all) {
                    Some(s) => self.place(Key::of(s), window, cx),
                    None => self.report("No agent has reported since you looked.", false, None),
                }
                return;
            }
            Action::Find => {
                if self.is_open(surface::FIND) {
                    self.close(surface::FIND, cx);
                } else {
                    self.open(Surface::Find, window, cx);
                }
                return;
            }
        }
        self.sync(cx);
        self.save(cx);
        if self.surfaces.is_empty() && self.shown {
            window.focus(&self.term_focus, cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_app_kit::chords::{KeyLike, after_leader, direct, filter, rows};

    fn alt(key: &str, ch: &str, shift: bool) -> KeyLike {
        KeyLike {
            key: key.into(),
            ch: Some(ch.into()),
            alt: true,
            shift,
            ..Default::default()
        }
    }

    fn bare(key: &str, ch: &str) -> KeyLike {
        KeyLike {
            key: key.into(),
            ch: Some(ch.into()),
            ..Default::default()
        }
    }

    /// Every terminal chord in CHROTE's useAppChords.ts, Alt form and
    /// leader key, and what Parvi does with it. `None` is the app's or no
    /// page's. Work owns leader B / Alt+B (column rather than a separate
    /// Beads tab), leader I / Alt+I (close its beside card), and leader R /
    /// Alt+R (report); Brain owns leader L / Alt+L. The shell owns leader ? / Alt+K and leader K.
    /// Files owns leader O / Alt+O instead of CHROTE's leader F / Alt+O,
    /// leaving Alt+F to the shell. There is no Agents tab or resident, so
    /// their G and Enter chords have no action.
    #[test]
    fn every_chrote_chord_has_its_place() {
        let all = chords(&WeakEntity::new_invalid(), 6);
        let direct_map = [
            ("i", false, None),
            ("enter", false, None),
            ("c", false, Some("Claim all sessions")),
            ("r", false, None),
            ("w", false, Some("Next window")),
            ("w", true, Some("Previous window")),
            ("=", false, Some("Add a window")),
            // A Finnish keyboard types + unshifted.
            ("+", false, Some("Add a window")),
            ("-", false, Some("Remove the last empty window")),
            ("n", false, Some("Launcher in the focused window")),
            ("a", false, Some("Sessions panel")),
            ("s", false, Some("Send from the focused session")),
            ("2", false, Some("Terminal tab 2")),
            ("6", false, Some("Terminal tab 6")),
            ("j", false, Some("Go to the session that reported")),
            // Alt+F is the shell's forward-word.
            ("f", false, None),
        ];
        for (key, shift, want) in direct_map {
            let ch = if shift {
                key.to_uppercase()
            } else {
                key.to_string()
            };
            let got = direct(&alt(key, &ch, shift), &all).map(|r| r.label);
            assert_eq!(got.as_deref(), want, "Alt+{key} shift={shift}");
        }
        let leader_map = [
            ("c", Some("Claim all sessions")),
            ("w", Some("Next window")),
            ("W", Some("Previous window")),
            ("=", Some("Add a window")),
            ("-", Some("Remove the last empty window")),
            ("/", Some("Session search")),
            ("n", Some("Launcher in the focused window")),
            ("s", Some("Send from the focused session")),
            ("3", Some("Terminal tab 3")),
            ("j", Some("Go to the session that reported")),
            ("i", None),
            ("r", None),
        ];
        for (ch, want) in leader_map {
            let got = after_leader(&bare(&ch.to_lowercase(), ch), &all).map(|r| r.label);
            assert_eq!(got.as_deref(), want, "leader {ch}");
        }
        assert_eq!(
            after_leader(&bare("tab", "\t"), &all)
                .map(|r| r.label)
                .as_deref(),
            Some("Sessions panel")
        );
        assert!(
            after_leader(&bare("enter", "\r"), &all).is_none(),
            "no resident yet"
        );
    }

    #[test]
    fn a_tab_past_the_shown_ones_is_a_no_op_chord() {
        let all = chords(&WeakEntity::new_invalid(), 1);
        assert_eq!(
            direct(&alt("2", "2", false), &all).unwrap().label,
            "Terminal tab 2 (not open)"
        );
        assert_eq!(
            direct(&alt("1", "1", false), &all).unwrap().caps,
            ["ALT", "1"]
        );
    }

    #[test]
    fn the_panel_lists_tabs_first_and_filters_by_words() {
        let r = rows(&chords(&WeakEntity::new_invalid(), 3));
        assert_eq!(r[0].label, "Terminal tab 1");
        assert_eq!(filter(&r, "window add").len(), 1);
        assert_eq!(filter(&r, "alt shift").len(), 1);
    }
}
