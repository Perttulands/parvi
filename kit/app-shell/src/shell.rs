//! The shell: the window around the pages. It holds the pages the app
//! registers, shows one at a time, and carries out what the hub tells it:
//! a frame to the page whose kinds hold it, an ask to the page that
//! answers it, the keyboard to the top layer. It knows no page by name.

use crate::close::{self, CloseGuard};
use crate::keys::KeysPanel;
use crate::settings_page::SettingsPage;
use crate::view_control::{self, ViewControl};
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, KeyDownEvent, Subscription, Task, Window,
};
use parvi_app_kit::chords::{self, Chord, KeyLike, Keyboard, Run, Verdict};
use parvi_app_kit::host::{Cover, Layer};
use parvi_app_kit::hub::{HubEvent, Surface};
use parvi_app_kit::page::Meta;
use parvi_app_kit::{Build, Error, Host, Hub, Mounted, declared, mount as mount_page};
use parvi_core::frame::{overlap, reserved};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// How long the keys a chord was typed with stay on screen.
const ECHO: Duration = Duration::from_millis(900);

pub struct Shell {
    pub(crate) hub: Entity<Hub>,
    pub(crate) host: Host,
    pub(crate) pages: Vec<Mounted>,
    pub(crate) shown: Option<usize>,
    pub(crate) focus: FocusHandle,
    leader: Option<Instant>,
    leader_task: Option<Task<()>>,
    pub(crate) echo: Option<Vec<String>>,
    echo_task: Option<Task<()>>,
    pub(crate) keys: Entity<KeysPanel>,
    /// Pages whose drawing failed, and why, until Try again.
    pub(crate) failed: HashMap<&'static str, String>,
    pub(crate) view: ViewControl,
    /// Pages whose channels are on: the one shown, and those docked.
    channels_on: HashSet<&'static str>,
    /// The operator chose to close the window over unsaved work.
    leaving: bool,
    /// Each page's own chord, made once from its key: the page, the
    /// leader's key, the Alt key and whether Shift goes with Alt.
    page_keys: Vec<(usize, &'static str, &'static str, bool)>,
    /// The status line's next redraw, when its words go stale.
    pub(crate) status_expiry: Option<(std::time::SystemTime, Task<()>)>,
    _subscriptions: Vec<Subscription>,
}

/// Makes every page, then the Settings page listing what they declare,
/// refusing two pages of one id or two that take the same frame kinds, any
/// that takes a reserved kind (`parvi_core::frame::RESERVED`) or declares a
/// setting twice, and two whose chords live everywhere share a key.
pub fn mount(
    builds: &[Build],
    hub: &Entity<Hub>,
    window: &mut Window,
    cx: &mut App,
) -> Result<Vec<Mounted>, Error> {
    let mut pages: Vec<Mounted> = builds
        .iter()
        .map(|b| b(Host::new(hub), window, cx))
        .collect();
    let declaring: Vec<Meta> = pages.iter().map(|p| p.meta).collect();
    let settings_hub = hub.clone();
    pages.push(mount_page(
        Host::new(hub),
        window,
        cx,
        |host, window, cx| SettingsPage::new(settings_hub, host, declaring, window, cx),
    ));
    let metas: Vec<Meta> = pages.iter().map(|p| p.meta).collect();
    check(&metas)?;
    let chords: Vec<_> = pages.iter().map(|p| (p.meta.id, p.chords(cx))).collect();
    check_chords(&chords)?;
    Ok(pages)
}

/// The rule [`mount`] keeps.
pub fn check(pages: &[Meta]) -> Result<(), Error> {
    for (i, a) in pages.iter().enumerate() {
        if let Some(owner) = reserved(a.kinds) {
            return Err(Error::new(
                "kinds_taken",
                format!("{} takes frame kinds of {owner}", a.id),
            ));
        }
        if let Some(key) = declared::twice(a.settings) {
            return Err(Error::new(
                "setting_twice",
                format!("{} declares its setting {key} twice", a.id),
            ));
        }
        for b in &pages[i + 1..] {
            if a.id == b.id {
                return Err(Error::new(
                    "page_twice",
                    format!("two pages are both {}", a.id),
                ));
            }
            if a.key.is_some() && a.key == b.key {
                return Err(Error::new(
                    "key_twice",
                    format!("{} and {} take the same key", a.id, b.id),
                ));
            }
            if overlap(a.kinds, b.kinds) {
                return Err(Error::new(
                    "kinds_overlap",
                    format!("{} and {} take the same frame kinds", a.id, b.id),
                ));
            }
        }
    }
    Ok(())
}

/// Chords live everywhere are live together, so two pages' must not share
/// a key: refused, naming both pages and the chords.
pub fn check_chords(pages: &[(&'static str, Vec<Chord>)]) -> Result<(), Error> {
    let everywhere = |chords: &[Chord]| -> Vec<Chord> {
        chords.iter().filter(|c| c.everywhere).cloned().collect()
    };
    for (i, (a, mine)) in pages.iter().enumerate() {
        for (b, theirs) in &pages[i + 1..] {
            for x in everywhere(mine) {
                if let Some(y) = everywhere(theirs).iter().find(|y| x.clashes(y)) {
                    return Err(Error::new(
                        "chords_twice",
                        format!("{a}'s {} and {b}'s {} take the same key", x.label, y.label),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// A page's key as its chord takes it: the leader's key, the Alt key, and
/// whether Shift goes with Alt. A letter is Alt+Shift and the letter, or
/// the leader then the capital; any other key is Alt and the key.
fn page_key(key: char) -> (&'static str, &'static str, bool) {
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    if key.is_ascii_alphabetic() {
        (
            leak(key.to_ascii_uppercase().to_string()),
            leak(key.to_ascii_lowercase().to_string()),
            true,
        )
    } else {
        let key = leak(key.to_string());
        (key, key, false)
    }
}

impl Shell {
    pub fn new(
        hub: Entity<Hub>,
        pages: Vec<Mounted>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let host = Host::new(&hub);
        let mut subscriptions = vec![
            cx.subscribe_in(&hub, window, Self::hub_event),
            cx.observe(&hub, |_, _, cx| cx.notify()),
        ];
        let me = cx.entity_id();
        subscriptions.extend(pages.iter().map(|p| p.redraws(me, cx)));
        let answers = pages.iter().map(|p| (p.meta.id, p.meta.answers)).collect();
        hub.update(cx, |h, _| h.set_answers(answers));
        let keys = cx.new(|cx| KeysPanel::new(host.clone(), window, cx));
        let page_keys = pages
            .iter()
            .enumerate()
            .filter_map(|(i, p)| Some((i, page_key(p.meta.key?))))
            .map(|(i, (leader, alt, shift))| (i, leader, alt, shift))
            .collect();
        if !pages.is_empty() {
            cx.defer_in(window, |s, window, cx| s.show(0, window, cx));
        }
        Self {
            hub,
            host,
            pages,
            shown: None,
            focus: cx.focus_handle(),
            leader: None,
            leader_task: None,
            echo: None,
            echo_task: None,
            keys,
            failed: HashMap::new(),
            view: ViewControl::default(),
            channels_on: HashSet::new(),
            leaving: false,
            page_keys,
            status_expiry: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn hub(&self) -> &Entity<Hub> {
        &self.hub
    }

    /// The id of the page shown, if any.
    pub fn shown_page(&self) -> Option<&'static str> {
        self.shown.map(|i| self.pages[i].meta.id)
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.pages.iter().position(|p| p.meta.id == id)
    }

    /// Shows page `i`: the one shown before hides and turns its channels
    /// off unless its dock is open, this one turns its on.
    pub fn show(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown == Some(i) {
            return;
        }
        let old = self.shown.replace(i);
        if let Some(old) = old {
            self.pages[old].shown(false, window, cx);
            self.sync_channels(old, cx);
        }
        self.sync_channels(i, cx);
        self.pages[i].shown(true, window, cx);
        self.refocus(window, cx);
        cx.notify();
    }

    /// Turns page `i`'s channels on while it shows or its dock is open,
    /// and off otherwise.
    fn sync_channels(&mut self, i: usize, cx: &mut Context<Self>) {
        let meta = self.pages[i].meta;
        let wanted = self.shown == Some(i) || self.hub.read(cx).docked(meta.id);
        let on = self.channels_on.contains(meta.id);
        if wanted == on {
            return;
        }
        if wanted {
            self.channels_on.insert(meta.id);
        } else {
            self.channels_on.remove(meta.id);
        }
        for kind in meta.channels {
            self.host.channel(*kind, wanted, cx);
        }
    }

    /// Shows the page with this id. False when there is none.
    pub fn show_id(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.index_of(id) {
            Some(i) => {
                self.show(i, window, cx);
                true
            }
            None => false,
        }
    }

    fn hub_event(
        &mut self,
        _: &Entity<Hub>,
        ev: &HubEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            HubEvent::Frame { node, kind, body } => {
                if *kind == view_control::VIEW_DO {
                    self.view_do(node.as_str(), body, window, cx);
                } else if let Some(p) = self.pages.iter().find(|p| p.meta.takes(*kind)) {
                    p.frame(node, *kind, body, window, cx);
                } else {
                    log::warn!("a frame of kind {kind:#04x} from {node} that no page takes");
                }
            }
            HubEvent::Ask { to, from, ask } => {
                if let Some(i) = self.index_of(to) {
                    if !ask.beside() {
                        self.show(i, window, cx);
                    }
                    self.pages[i].answer(ask.clone(), from, window, cx);
                }
            }
            HubEvent::Show(id) => {
                self.show_id(id, window, cx);
            }
            HubEvent::Dock(id) => {
                if let Some(i) = self.index_of(id) {
                    self.sync_channels(i, cx);
                }
            }
            HubEvent::SettingChanged(id) => {
                if let Some(i) = self.index_of(id) {
                    self.pages[i].settings_changed(window, cx);
                }
            }
            HubEvent::SettingsReloaded(changed) => {
                // The Settings page shows what the file holds now.
                let settings = std::iter::once(&crate::settings_page::ID);
                for id in changed.iter().chain(settings) {
                    if let Some(i) = self.index_of(id) {
                        self.pages[i].settings_changed(window, cx);
                    }
                }
            }
            HubEvent::Layers { closed } => {
                for s in closed {
                    if let Surface::Layer(Layer {
                        dismiss: Some(run), ..
                    }) = s
                    {
                        run(window, cx);
                    }
                }
                self.refocus(window, cx);
            }
            HubEvent::Link { node, state } => {
                for p in &self.pages {
                    p.link(node, *state, window, cx);
                }
            }
        }
        cx.notify();
    }

    /// Whether the window may close: yes when no guarded view holds unsaved
    /// work, or the operator chose to leave it. Otherwise the page holding
    /// the first comes forward and the close guard asks over the window.
    pub fn may_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.leaving {
            return true;
        }
        let unsaved = self.hub.read(cx).unsaved(cx);
        let Some((page, _)) = unsaved.first() else {
            return true;
        };
        self.show_id(page, window, cx);
        let labels = unsaved.into_iter().map(|(_, label)| label).collect();
        let me = cx.weak_entity();
        let host = self.host.clone();
        let guard = cx.new(|cx| CloseGuard::new(me, host, labels, cx));
        let focus = guard.read(cx).focus();
        let layer = Layer::new(close::LAYER, guard)
            .cover(Cover::Window)
            .focus(focus);
        self.host.layer(layer, cx);
        false
    }

    /// Closes the window over the unsaved work, as the operator chose.
    pub fn leave(&mut self, window: &mut Window) {
        self.leaving = true;
        window.remove_window();
    }

    /// The keyboard goes to the top layer that takes it, or to the page.
    fn refocus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let top = self.hub.read(cx).stack.iter().rev().find_map(|s| match s {
            Surface::Layer(l) => l.focus.clone(),
            Surface::Menu(_) => None,
        });
        let page = || {
            self.shown
                .and_then(|i| self.pages[i].focus(cx))
                .unwrap_or_else(|| self.focus.clone())
        };
        let to = top.unwrap_or_else(page);
        window.focus(&to, cx);
    }

    /// The chords live now: the shell's, those of pages not shown that are
    /// live everywhere, then the shown page's, which win a key both take.
    pub fn chords(&self, cx: &mut Context<Self>) -> Vec<Chord> {
        let shell = cx.entity().downgrade();
        let hub = self.hub.downgrade();
        let mut all = vec![
            Chord::new("?", "Keybindings", move |window, cx| {
                let _ = shell.update(cx, |s, cx| s.toggle_keys(window, cx));
            })
            .alt("k", Some(false)),
            Chord::new("k", "Keys off", move |_, cx| {
                let _ = hub.update(cx, |h, cx| {
                    h.settings.keys = false;
                    h.save(cx);
                    h.toast("Keys off", cx);
                });
            }),
        ];
        for &(i, leader, alt, shift) in &self.page_keys {
            let shell = cx.entity().downgrade();
            all.push(
                Chord::new(
                    leader,
                    format!("Show {}", self.pages[i].meta.title),
                    move |window, cx| {
                        let _ = shell.update(cx, |s, cx| s.show(i, window, cx));
                    },
                )
                .alt(alt, Some(shift)),
            );
        }
        for (i, page) in self.pages.iter().enumerate() {
            if self.shown != Some(i) {
                all.extend(page.chords(cx).into_iter().filter(|c| c.everywhere));
            }
        }
        if let Some(i) = self.shown {
            all.extend(self.pages[i].chords(cx));
        }
        all
    }

    pub(crate) fn toggle_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.is_open(crate::keys::LAYER, cx) {
            self.host.close_layer(crate::keys::LAYER, cx);
        } else {
            self.open_keys(window, cx);
        }
    }

    fn open_keys(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let chords = self.chords(cx);
        let was_open = self.host.is_open(crate::keys::LAYER, cx);
        let focus = self.keys.update(cx, |k, cx| {
            k.set_chords(chords, !was_open, cx);
            k.focus(cx)
        });
        let layer = Layer::new(crate::keys::LAYER, self.keys.clone())
            .cover(Cover::Window)
            .focus(focus);
        self.host.layer(layer, cx);
    }

    fn key_like(e: &KeyDownEvent) -> KeyLike {
        let k = &e.keystroke;
        KeyLike {
            key: k.key.clone(),
            ch: k.key_char.clone(),
            alt: k.modifiers.alt,
            ctrl: k.modifiers.control,
            shift: k.modifiers.shift,
            platform: k.modifiers.platform,
        }
    }

    /// Every key passes here first, wherever focus is; `chords::route`
    /// decides what becomes of it.
    pub fn capture_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = Self::key_like(e);
        let all = self.chords(cx);
        let hub = self.hub.read(cx);
        let keyboard = Keyboard {
            on: hub.settings.keys,
            leader: self.leader,
            surfaces: !hub.stack.is_empty(),
        };
        match chords::route(&k, &keyboard, &all, Instant::now()) {
            Verdict::Pass => {}
            Verdict::Dismiss => {
                cx.stop_propagation();
                if chords::leader_waiting(self.leader, Instant::now()) {
                    self.leader = None;
                }
                self.hub.update(cx, |h, cx| h.dismiss(cx));
            }
            Verdict::KeysOffHint => {
                // The leader still answers with keys off, to say how to turn
                // them back on, and nothing else.
                self.host.report(
                    parvi_app_kit::host::Status::new(crate::chrome::KEYS_OFF_HINT),
                    cx,
                );
            }
            Verdict::OpenLeader => {
                cx.stop_propagation();
                self.open_leader(window, cx);
            }
            Verdict::AfterLeader(row) => {
                self.leader = None;
                self.keys.update(cx, |k, cx| k.set_leader(false, cx));
                // A key no chord takes starts the panel's search.
                if let Some(row) = row {
                    cx.stop_propagation();
                    let key = if row.leader == "tab" {
                        "TAB".to_string()
                    } else {
                        row.leader.to_uppercase()
                    };
                    // The first chord is the panel's own, which stays.
                    if row.chord != 0 {
                        self.host.close_layer(crate::keys::LAYER, cx);
                    }
                    self.run(
                        all[row.chord].run.clone(),
                        vec!["LEADER".into(), key],
                        window,
                        cx,
                    );
                }
            }
            Verdict::Direct(row) => {
                if e.prefer_character_input {
                    return;
                }
                cx.stop_propagation();
                self.leader = None;
                self.run(all[row.chord].run.clone(), row.caps, window, cx);
            }
        }
        cx.notify();
    }

    /// Runs a chord once this update is over, so it may use anything,
    /// the shell included, and shows the keys it was typed with.
    pub(crate) fn run(
        &mut self,
        run: Run,
        caps: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_echo(caps, cx);
        window.defer(cx, move |window, cx| run(window, cx));
    }

    /// The leader: the keys panel opens, and the next bare key runs a chord.
    fn open_leader(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leader = Some(Instant::now());
        self.show_echo(vec!["CTRL+SHIFT".into(), "SPACE".into()], cx);
        self.open_keys(window, cx);
        self.keys.update(cx, |k, cx| k.set_leader(true, cx));
        self.leader_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(chords::LEADER_WINDOW).await;
            let _ = this.update(cx, |s, cx| {
                if !chords::leader_waiting(s.leader, Instant::now()) {
                    s.leader = None;
                    s.keys.update(cx, |k, cx| k.set_leader(false, cx));
                    cx.notify();
                }
            });
        }));
    }

    fn show_echo(&mut self, caps: Vec<String>, cx: &mut Context<Self>) {
        self.echo = Some(caps);
        self.echo_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ECHO).await;
            let _ = this.update(cx, |s, cx| {
                s.echo = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl gpui::Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &'static str, kinds: parvi_core::frame::Kinds) -> Meta {
        Meta {
            id,
            title: id,
            key: None,
            area: None,
            kinds,
            channels: &[],
            answers: &[],
            settings: &[],
        }
    }

    #[test]
    fn pages_that_clash_refuse_to_start_naming_both() {
        let ok = [meta("a", &[0xc0..=0xc3]), meta("b", &[0xc4..=0xc4])];
        assert!(check(&ok).is_ok());
        let twice = check(&[meta("a", &[]), meta("a", &[])]).unwrap_err();
        assert_eq!(twice.message, "two pages are both a");
        let overlap = check(&[meta("a", &[0xc0..=0xc3]), meta("b", &[0xc3..=0xc3])]).unwrap_err();
        assert_eq!(overlap.message, "a and b take the same frame kinds");
        let link = check(&[meta("a", &[0x00..=0x01])]).unwrap_err();
        assert_eq!(link.message, "a takes frame kinds of the node's hello");
        let fleet = check(&[meta("a", &[0x1b..=0x1b])]).unwrap_err();
        assert_eq!(
            fleet.message,
            "a takes frame kinds of the fleet's peer list"
        );
        let shell = check(&[meta("a", &[0xb3..=0xb3])]).unwrap_err();
        assert_eq!(shell.message, "a takes frame kinds of view control");
        const LOUD: parvi_app_kit::Setting = parvi_app_kit::Setting::toggle("loud", "Loud", false);
        let twice = Meta {
            settings: &[LOUD, LOUD],
            ..meta("a", &[])
        };
        let e = check(&[twice]).unwrap_err();
        assert_eq!(e.message, "a declares its setting loud twice");
        let keyed = |id, key| Meta {
            key: Some(key),
            ..meta(id, &[])
        };
        assert!(check(&[keyed("a", 'A'), keyed("b", 'B')]).is_ok());
        let e = check(&[keyed("a", 'A'), keyed("b", 'A')]).unwrap_err();
        assert_eq!(e.message, "a and b take the same key");
    }

    #[test]
    fn a_page_key_is_alt_shift_and_its_letter_or_alt_and_its_key() {
        assert_eq!(page_key('A'), ("A", "a", true));
        assert_eq!(page_key(','), (",", ",", false));
    }

    #[test]
    fn chords_two_pages_keep_everywhere_refuse_to_share_a_key() {
        let chord = |leader, label: &str| Chord::new(leader, label, |_, _| {});
        let a = vec![
            chord("t", "Tabs").everywhere().alt("1", None),
            chord("x", "Only here"),
        ];
        let fine = vec![
            chord("u", "Up").everywhere().alt("2", Some(false)),
            chord("t", "Its own t, only while shown"),
        ];
        assert!(check_chords(&[("a", a.clone()), ("b", fine)]).is_ok());
        let by_leader = vec![chord("t", "Toast").everywhere()];
        let e = check_chords(&[("a", a.clone()), ("b", by_leader)]).unwrap_err();
        assert_eq!(e.code, "chords_twice");
        assert_eq!(e.message, "a's Tabs and b's Toast take the same key");
        let by_alt = vec![chord("y", "One").everywhere().alt("1", Some(true))];
        assert!(check_chords(&[("a", a.clone()), ("b", by_alt)]).is_err());
        let shifted = vec![chord("y", "Shifted").everywhere().alt("w", Some(true))];
        let plain = vec![chord("z", "Plain").everywhere().alt("w", Some(false))];
        assert!(check_chords(&[("a", shifted), ("b", plain)]).is_ok());
    }
}
