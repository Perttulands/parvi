//! The keyboard (CHROTE's keys/chords.ts and useAppChords.ts). A focused
//! terminal gets every key, so Parvi takes one modifier: Alt. A registered
//! Alt chord runs wherever focus is; every other Alt combination reaches
//! the program, and so does AltGr (Ctrl+Alt on Windows), which a Finnish
//! layout needs for `@`, `$`, `{` and `}`. The leader, Ctrl+Shift+Space,
//! opens the keys panel and takes one bare key for 1.5 s. Keys off means
//! nothing is intercepted, except the leader's own way back.
//!
//! [`route`] is the one decision every key passes: the shell only carries
//! out its verdict.

use gpui::{App, Window};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// What a chord does.
pub type Run = Rc<dyn Fn(&mut Window, &mut App)>;

/// A chord a page or the shell registers: its leader key, its Alt key if
/// it has one, what the keys panel calls it, and what it does.
#[derive(Clone)]
pub struct Chord {
    /// The bare key after the leader.
    pub leader: &'static str,
    pub direct: Option<Direct>,
    pub label: String,
    pub run: Run,
}

impl Chord {
    pub fn new(
        leader: &'static str,
        label: impl Into<String>,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            leader,
            direct: None,
            label: label.into(),
            run: Rc::new(run),
        }
    }

    /// Alt and `key`, Shift as `shift` says (`None` where the layout
    /// decides whether the key needs Shift).
    pub fn alt(mut self, key: &'static str, shift: Option<bool>) -> Self {
        self.direct = Some(Direct {
            key,
            shift,
            layout: &[],
        });
        self
    }

    /// Other keys a layout types the Alt key's character with.
    pub fn or_layout(mut self, layout: &'static [&'static str]) -> Self {
        if let Some(d) = &mut self.direct {
            d.layout = layout;
        }
        self
    }
}

pub const LEADER_LABEL: &str = "Ctrl+Shift+Space";
/// How long the leader waits for its key with nothing pressed.
pub const LEADER_WINDOW: Duration = Duration::from_millis(1500);

/// A key as the platform reports it: `key` is the layout's key name in
/// lower case ("a", "=", "+", "enter", "tab"), `ch` the character it types.
#[derive(Clone, Debug, Default)]
pub struct KeyLike {
    pub key: String,
    pub ch: Option<String>,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub platform: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Direct {
    key: &'static str,
    /// `None` where the layout decides whether the key needs Shift.
    shift: Option<bool>,
    layout: &'static [&'static str],
}

/// A chord as the keys panel lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Which chord, in the order they were registered.
    pub chord: usize,
    /// Key caps for the panel and the echo, modifier first.
    pub caps: Vec<String>,
    /// The bare key after the leader.
    pub leader: String,
    pub label: String,
}

fn cap(key: &str) -> String {
    match key {
        "+" | "=" => "PLUS".into(),
        "-" => "MINUS".into(),
        "tab" => "TAB".into(),
        k => k.to_uppercase(),
    }
}

fn direct_caps(d: &Direct) -> Vec<String> {
    let mut caps = vec!["ALT".to_string()];
    if d.shift == Some(true) {
        caps.push("SHIFT".into());
    }
    caps.push(cap(d.key));
    caps
}

/// Every chord in scope, for the keys panel, in the order registered.
pub fn rows(chords: &[Chord]) -> Vec<Row> {
    chords
        .iter()
        .enumerate()
        .map(|(i, c)| Row {
            chord: i,
            caps: c.direct.as_ref().map(direct_caps).unwrap_or_default(),
            leader: c.leader.into(),
            label: c.label.clone(),
        })
        .collect()
}

pub fn is_leader(k: &KeyLike) -> bool {
    k.ctrl && k.shift && !k.alt && !k.platform && k.key == "space"
}

fn same_key(wanted: &str, k: &KeyLike) -> bool {
    let ch = k.ch.as_deref().unwrap_or("");
    if wanted.len() == 1 && wanted.chars().all(|c| c.is_ascii_alphabetic()) {
        return k.key.eq_ignore_ascii_case(wanted);
    }
    k.key == wanted || ch == wanted
}

/// The Alt chord a key is, or `None` if the key belongs to the page.
/// Of two chords on one key, the later registered wins.
pub fn direct(k: &KeyLike, chords: &[Chord]) -> Option<Row> {
    if !k.alt || k.ctrl || k.platform {
        return None;
    }
    let i = chords.iter().rposition(|c| {
        c.direct.as_ref().is_some_and(|d| {
            d.shift.is_none_or(|s| s == k.shift)
                && std::iter::once(d.key)
                    .chain(d.layout.iter().copied())
                    .any(|w| same_key(w, k))
        })
    })?;
    rows(chords).into_iter().nth(i)
}

/// The chord a bare key after the leader runs: an exact key beats a
/// case-insensitive one, which is how `w` and `W` are two chords.
pub fn after_leader(k: &KeyLike, chords: &[Chord]) -> Option<Row> {
    let all = rows(chords);
    let typed = match k.ch.as_deref() {
        Some(c) if !c.is_empty() && k.key != "tab" => c.to_string(),
        _ => k.key.clone(),
    };
    if let Some(r) = all.iter().find(|r| r.leader == typed) {
        return Some(r.clone());
    }
    all.into_iter()
        .find(|r| r.leader.eq_ignore_ascii_case(&typed))
}

/// What the app's keyboard is doing when a key arrives.
#[derive(Clone, Copy, Debug)]
pub struct Keyboard {
    /// Keys on: chords and the leader are the app's.
    pub on: bool,
    /// When the leader was pressed, if it was.
    pub leader: Option<Instant>,
    /// A layer is open, so Escape is its.
    pub surfaces: bool,
}

/// What becomes of a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Escape: the leader stops waiting, or the top layer closes.
    Dismiss,
    OpenLeader,
    /// The leader with keys off: it only says how to turn them on.
    KeysOffHint,
    /// The leader's bare key: its chord, or `None` for a key the panel's
    /// search takes.
    AfterLeader(Option<Row>),
    Direct(Row),
    /// The focused control's, or the page's.
    Pass,
}

/// Whether the leader still waits for its key.
pub fn leader_waiting(pressed: Option<Instant>, now: Instant) -> bool {
    pressed.is_some_and(|t| now.saturating_duration_since(t) < LEADER_WINDOW)
}

/// A modifier's own press is not the leader's next key: holding Shift to
/// reach `?` or `W` must not end the wait.
fn is_modifier(key: &str) -> bool {
    matches!(
        key,
        "shift" | "control" | "alt" | "platform" | "function" | "capslock" | "altgr"
    )
}

/// The one decision for every key, wherever focus is.
pub fn route(k: &KeyLike, s: &Keyboard, chords: &[Chord], now: Instant) -> Verdict {
    let waiting = s.on && leader_waiting(s.leader, now);
    let plain = !k.alt && !k.ctrl && !k.shift && !k.platform;
    if plain && k.key == "escape" && (waiting || s.surfaces) {
        return Verdict::Dismiss;
    }
    if is_leader(k) {
        return if s.on {
            Verdict::OpenLeader
        } else {
            Verdict::KeysOffHint
        };
    }
    if !s.on {
        return Verdict::Pass;
    }
    // Asked before the leader's key, as in CHROTE: holding Alt means the
    // direct chord, so Alt+K is the panel even while a bare `k` would be
    // keys off.
    if let Some(row) = direct(k, chords) {
        return Verdict::Direct(row);
    }
    if waiting && !is_modifier(&k.key) {
        return Verdict::AfterLeader(after_leader(k, chords));
    }
    Verdict::Pass
}

/// Rows whose label or caps hold every word typed, in any order.
pub fn filter(list: &[Row], query: &str) -> Vec<Row> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    list.iter()
        .filter(|r| {
            let text = format!("{} {} {}", r.caps.join("+"), r.leader, r.label).to_lowercase();
            words.iter().all(|w| text.contains(w.as_str()))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chords of the shapes pages register: a letter, one with and without
    /// Shift, a key layouts type differently, leader-only keys, `tab`, and
    /// numbered tabs.
    fn table() -> Vec<Chord> {
        let c = |leader, label: &str| Chord::new(leader, label, |_, _| {});
        let mut list: Vec<Chord> = ["1", "2", "3", "4", "5", "6"]
            .into_iter()
            .map(|n| c(n, &format!("Tab {n}")).alt(n, Some(false)))
            .collect();
        list.extend([
            c("?", "Keybindings").alt("k", Some(false)),
            c("k", "Keys off"),
            c("w", "Next window").alt("w", Some(false)),
            c("W", "Previous window").alt("w", Some(true)),
            c("=", "Add a window").alt("+", None).or_layout(&["="]),
            c("-", "Remove a window").alt("-", None),
            c("/", "Session search"),
            c("n", "Launcher").alt("n", Some(false)),
            c("tab", "Sessions panel").alt("a", Some(false)),
        ]);
        list
    }

    fn alt(key: &str, ch: &str, shift: bool) -> KeyLike {
        KeyLike {
            key: key.into(),
            ch: Some(ch.into()),
            alt: true,
            shift,
            ..Default::default()
        }
    }

    fn label(r: Option<Row>) -> Option<String> {
        r.map(|r| r.label)
    }

    fn is(v: &Verdict, want: &str) -> bool {
        matches!(v, Verdict::Direct(r) | Verdict::AfterLeader(Some(r)) if r.label == want)
    }

    #[test]
    fn alt_chords_find_their_chord() {
        let t = table();
        let got = |k| label(direct(&k, &t));
        assert_eq!(got(alt("n", "n", false)).as_deref(), Some("Launcher"));
        assert_eq!(got(alt("w", "W", true)).as_deref(), Some("Previous window"));
        assert_eq!(got(alt("2", "2", false)).as_deref(), Some("Tab 2"));
        assert_eq!(got(alt("=", "=", false)).as_deref(), Some("Add a window"));
        // A Finnish keyboard types + unshifted.
        assert_eq!(got(alt("+", "+", false)).as_deref(), Some("Add a window"));
        assert_eq!(
            got(alt("-", "-", false)).as_deref(),
            Some("Remove a window")
        );
        assert_eq!(
            direct(&alt("k", "k", false), &t).unwrap().caps,
            ["ALT", "K"]
        );
    }

    #[test]
    fn other_keys_are_the_programs() {
        let t = table();
        // Alt+F is the shell's forward-word; Alt+7 has no chord.
        assert!(direct(&alt("f", "f", false), &t).is_none());
        assert!(direct(&alt("7", "7", false), &t).is_none());
        // AltGr arrives as Ctrl+Alt: Finnish @ is AltGr+2.
        let altgr = KeyLike {
            ctrl: true,
            ..alt("2", "@", false)
        };
        assert!(direct(&altgr, &t).is_none());
        let plain = KeyLike {
            key: "n".into(),
            ch: Some("n".into()),
            ..Default::default()
        };
        assert!(direct(&plain, &t).is_none());
    }

    #[test]
    fn the_leader_takes_a_bare_key() {
        let t = table();
        let leader = KeyLike {
            key: "space".into(),
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        assert!(is_leader(&leader));
        let got = |key: &str, ch: &str| label(after_leader(&bare(key, ch), &t));
        assert_eq!(got("/", "/").as_deref(), Some("Session search"));
        assert_eq!(got("w", "W").as_deref(), Some("Previous window"));
        assert_eq!(got("w", "w").as_deref(), Some("Next window"));
        assert_eq!(got("k", "k").as_deref(), Some("Keys off"));
        assert_eq!(got("tab", "\t").as_deref(), Some("Sessions panel"));
        assert_eq!(got("3", "3").as_deref(), Some("Tab 3"));
        assert!(got("x", "x").is_none());
    }

    fn bare(key: &str, ch: &str) -> KeyLike {
        KeyLike {
            key: key.into(),
            ch: Some(ch.into()),
            ..Default::default()
        }
    }

    const KEYBOARD: Keyboard = Keyboard {
        on: true,
        leader: None,
        surfaces: false,
    };

    #[test]
    fn altgr_on_a_finnish_layout_types_into_the_terminal() {
        let t = table();
        let now = Instant::now();
        // Windows reports AltGr as Ctrl+Alt: @ { [ ] } \ $ on 2 7 8 9 0 + 4.
        let finnish = [
            ("2", "@"),
            ("7", "{"),
            ("8", "["),
            ("9", "]"),
            ("0", "}"),
            ("+", "\\"),
            ("4", "$"),
        ];
        for (key, ch) in finnish {
            let altgr = KeyLike {
                ctrl: true,
                ..alt(key, ch, false)
            };
            assert_eq!(
                route(&altgr, &KEYBOARD, &t, now),
                Verdict::Pass,
                "AltGr+{key}"
            );
            // X11 reports the character with no Alt at all.
            assert_eq!(route(&bare(key, ch), &KEYBOARD, &t, now), Verdict::Pass);
        }
        // Plain Alt on the same keys is still the app's.
        assert!(is(
            &route(&alt("2", "2", false), &KEYBOARD, &t, now),
            "Tab 2"
        ));
        assert!(is(
            &route(&alt("+", "+", false), &KEYBOARD, &t, now),
            "Add a window"
        ));
    }

    #[test]
    fn the_leader_waits_a_moment_for_one_key() {
        let t = table();
        let at = Instant::now();
        let waiting = Keyboard {
            leader: Some(at),
            ..KEYBOARD
        };
        let soon = at + Duration::from_millis(1499);
        let late = at + LEADER_WINDOW;
        let route = |k: &KeyLike, now| route(k, &waiting, &t, now);
        assert!(is(&route(&bare("n", "n"), soon), "Launcher"));
        assert_eq!(route(&bare("n", "n"), late), Verdict::Pass);
        // A key it doesn't list goes to the panel's search.
        assert_eq!(route(&bare("x", "x"), soon), Verdict::AfterLeader(None));
        // Holding Shift to reach W is not the next key.
        assert_eq!(route(&bare("shift", ""), soon), Verdict::Pass);
        let shift_w = KeyLike {
            shift: true,
            ..bare("w", "W")
        };
        assert!(is(&route(&shift_w, soon), "Previous window"));
        // Holding Alt means the direct chord: Alt+K is the panel, not keys off.
        assert!(matches!(
            route(&alt("k", "k", false), soon),
            Verdict::Direct(r) if r.label == "Keybindings"
        ));
        // Escape stops the wait; once it is over, Escape is the page's.
        assert_eq!(route(&bare("escape", ""), soon), Verdict::Dismiss);
        assert_eq!(route(&bare("escape", ""), late), Verdict::Pass);
    }

    #[test]
    fn keys_off_leaves_every_key_to_the_terminal() {
        let t = table();
        let now = Instant::now();
        let off = Keyboard {
            on: false,
            leader: Some(now),
            ..KEYBOARD
        };
        assert_eq!(route(&alt("n", "n", false), &off, &t, now), Verdict::Pass);
        assert_eq!(route(&alt("1", "1", false), &off, &t, now), Verdict::Pass);
        assert_eq!(route(&bare("k", "k"), &off, &t, now), Verdict::Pass);
        let leader = KeyLike {
            key: "space".into(),
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(route(&leader, &off, &t, now), Verdict::KeysOffHint);
        assert_eq!(route(&leader, &KEYBOARD, &t, now), Verdict::OpenLeader);
        // An open layer still closes on Escape.
        let open = Keyboard {
            surfaces: true,
            ..off
        };
        assert_eq!(route(&bare("escape", ""), &open, &t, now), Verdict::Dismiss);
    }

    #[test]
    fn escape_reaches_the_terminal_only_when_nothing_is_open() {
        let t = table();
        let now = Instant::now();
        let open = Keyboard {
            surfaces: true,
            ..KEYBOARD
        };
        assert_eq!(route(&bare("escape", ""), &open, &t, now), Verdict::Dismiss);
        assert_eq!(
            route(&bare("escape", ""), &KEYBOARD, &t, now),
            Verdict::Pass
        );
        let shifted = KeyLike {
            shift: true,
            ..bare("escape", "")
        };
        assert_eq!(route(&shifted, &open, &t, now), Verdict::Pass);
    }

    #[test]
    fn the_panel_lists_in_order_and_filters_by_words() {
        let r = rows(&table());
        assert_eq!(r[0].label, "Tab 1");
        assert_eq!(filter(&r, "window add").len(), 1);
        assert_eq!(filter(&r, "alt shift").len(), 1);
    }
}
