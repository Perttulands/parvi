//! What a session's menu and the rail's pick do beyond rename (#184): copy
//! a session, kill one session or the picked rows with one receipt, and
//! turn tmux's mouse mode on or off on a device. CHROTE's `SessionItem`,
//! `SessionPanel` and `tmux_mouse.go` are the spec.

use super::launcher::notify_offered;
use super::page::{Act, MenuItem, Terminals};
use crate::store::selection::{Receipt, Sent};
use crate::store::sessions::Key;
use crate::store::{launch, session_copy};
use gpui::{Context, KeyDownEvent, Window};

/// How a click on a rail row changes the pick: Ctrl (or Cmd) flips it,
/// Shift adds a range, a plain click empties it and peeks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Toggle,
    Range,
    Plain,
}

impl Gesture {
    pub fn of(m: &gpui::Modifiers) -> Gesture {
        if m.control || m.platform {
            Gesture::Toggle
        } else if m.shift {
            Gesture::Range
        } else {
            Gesture::Plain
        }
    }
}

impl Terminals {
    /// Why Copy session is refused for this session, if it is.
    pub fn copy_refused(&self, k: &Key) -> Option<&'static str> {
        let s = self.fleet.session(k)?;
        let harnesses = self
            .fleet
            .get(&k.node)
            .and_then(|c| c.launcher.as_ref())
            .map(|l| &l.harnesses[..])
            .unwrap_or(&[]);
        session_copy::harness(s, harnesses).err()
    }

    /// Copy session as a menu row: refused, saying why, when it cannot.
    pub fn copy_row(&self, k: &Key) -> MenuItem {
        let row = MenuItem::new("Copy session", Act::Copy(k.clone()));
        match self.copy_refused(k) {
            Some(why) => row.refused(why),
            None => row,
        }
    }

    /// Launches what the session runs again, in its folder, on its node and
    /// Unix user, named next in its family.
    pub fn copy_session(&mut self, k: &Key) {
        let Some(s) = self.fleet.session(k).cloned() else {
            self.report(
                &format!("Could not copy {}: it is gone", k.name),
                true,
                None,
            );
            return;
        };
        let Some(c) = self.fleet.get(&k.node) else {
            return;
        };
        let conf = c.launcher.clone();
        let harnesses = conf.as_ref().map(|l| &l.harnesses[..]).unwrap_or(&[]);
        let harness = match session_copy::harness(&s, harnesses) {
            Ok(h) => h.clone(),
            Err(why) => {
                self.report(&format!("Could not copy {}: {why}", k.name), true, None);
                return;
            }
        };
        let taken: Vec<&str> = (c.sessions.iter())
            .filter(|o| o.user.as_deref().unwrap_or("") == k.user)
            .map(|o| o.name.as_str())
            .collect();
        let name = session_copy::next_name(&k.name, &taken);
        let launch = parvi_terminals_contract::Launch {
            harness: harness.id.clone(),
            folder: s.cwd.clone().unwrap_or_default(),
            name: name.clone(),
            flags: harness.flags.clone(),
            user: k.user.clone(),
            notify: self.settings.notify
                && notify_offered(&harness.command, &k.user)
                && conf.as_ref().is_some_and(|l| l.notify_off.is_none()),
            size: None,
        };
        match self.fleet.launch_on(&k.node, launch) {
            Some(id) => {
                let tab = self.settings.workspace.active;
                self.launches
                    .insert((k.node.clone(), id), (name.clone(), None, tab));
                self.report(&format!("Copying {} as {name}…", k.name), false, None);
            }
            None => self.report(
                &format!("Could not copy {}: its device is away", k.name),
                true,
                None,
            ),
        }
    }

    /// Starts a session another page asked for (`Ask::Launch`): on `node`,
    /// in `folder`, as `name` or the next free name in its family, with the
    /// harness last launched (else the node's first) and its kept flags.
    pub fn launch_asked(&mut self, node: &str, folder: &str, name: &str) {
        let Some(c) = self.fleet.get(node) else {
            let why = match self.where_is(node) {
                Some(p) => format!("{} is away", p.device),
                None => "its node is not in the fleet".into(),
            };
            self.report(&format!("Could not start {name}: {why}"), true, None);
            return;
        };
        let Some(conf) = c.launcher.clone() else {
            let why = "the node has not said what it can launch yet";
            self.report(&format!("Could not start {name}: {why}"), true, None);
            return;
        };
        let Some(harness) = (self.settings.last_harness.as_ref())
            .and_then(|id| conf.harnesses.iter().find(|h| &h.id == id))
            .or_else(|| conf.harnesses.first())
            .cloned()
        else {
            let why = "the node lists no harness";
            self.report(&format!("Could not start {name}: {why}"), true, None);
            return;
        };
        if let Some(why) = launch::name_problem(name) {
            self.report(&format!("Could not start {name}: {why}"), true, None);
            return;
        }
        let taken: Vec<&str> = (c.sessions.iter())
            .filter(|s| s.user.as_deref().unwrap_or("").is_empty())
            .map(|s| s.name.as_str())
            .collect();
        let name = if taken.contains(&name) {
            session_copy::next_name(name, &taken)
        } else {
            name.to_string()
        };
        let flags = (self.settings.flags.get(&harness.id).cloned())
            .unwrap_or_else(|| harness.flags.clone());
        let launch = parvi_terminals_contract::Launch {
            harness: harness.id.clone(),
            folder: folder.to_string(),
            name: name.clone(),
            flags,
            user: String::new(),
            notify: self.settings.notify
                && notify_offered(&harness.command, "")
                && conf.notify_off.is_none(),
            size: self.launch_grids.borrow()[self.settings.workspace.focus],
        };
        match self.fleet.launch_on(node, launch) {
            Some(id) => {
                let tab = self.settings.workspace.active;
                self.launches
                    .insert((node.to_string(), id), (name.clone(), None, tab));
                self.report(&format!("Starting {name} in {folder}…"), false, None);
            }
            None => self.report(
                &format!("Could not start {name}: its device is away"),
                true,
                None,
            ),
        }
    }

    /// Kills exactly these sessions, each by its id, and reports once they
    /// have all answered. `picked`: the rail's pick, counted in the receipt.
    pub fn kill_sessions(&mut self, keys: &[Key], picked: bool) {
        let (mut sent, mut refused) = (vec![], vec![]);
        for k in keys {
            let Some(s) = self.fleet.session(k).cloned() else {
                refused.push((k.name.clone(), "it is gone".to_string()));
                continue;
            };
            match self.fleet.delete(&k.node, &s.id) {
                Some(id) => sent.push(Sent {
                    node: k.node.clone(),
                    id,
                    key: k.text(),
                    name: s.name.clone(),
                }),
                None => refused.push((s.name.clone(), "its device is away".to_string())),
            }
        }
        let text = match (picked, &sent[..]) {
            (false, [one]) => format!("Killing {}…", one.name),
            (_, []) => String::new(),
            (_, many) => format!("Killing {} sessions…", many.len()),
        };
        if !text.is_empty() {
            self.report(&text, false, None);
        }
        if let Some(r) = self.kills.start(picked, sent, refused) {
            self.kill_receipt(r);
        }
    }

    /// A node answered a kill.
    pub fn killed(&mut self, node: &str, id: u32, ok: bool, text: &str) {
        if let Some(r) = self.kills.answered(node, id, ok, text) {
            self.kill_receipt(r);
        }
    }

    fn kill_receipt(&mut self, r: Receipt) {
        self.pick.without(&r.killed);
        self.report(&r.text, r.error, None);
    }

    /// The pick's rows the rail shows, in its order: what Kill N kills.
    pub fn pick_targets(&self) -> Vec<Key> {
        let shown: Vec<String> = self.rail_shown.iter().map(Key::text).collect();
        let targets = self.pick.targets(&shown);
        (self.rail_shown.iter())
            .filter(|k| targets.contains(&k.text()))
            .cloned()
            .collect()
    }

    /// The pick's menu: Kill N sessions, armed in place by the first click.
    pub fn pick_menu(&self) -> Vec<MenuItem> {
        let n = self.pick_targets().len();
        let what = if n == 1 {
            "1 session".to_string()
        } else {
            format!("{n} sessions")
        };
        vec![MenuItem::new(format!("Kill {what}"), Act::KillPicked).danger()]
    }

    /// A click on a rail row: the pick changes, and a plain click peeks.
    pub fn rail_click(
        &mut self,
        k: &Key,
        gesture: Gesture,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = k.text();
        match gesture {
            Gesture::Plain => {
                self.pick.plain(&key);
                self.open_peek(k.clone(), None, window, cx);
            }
            Gesture::Toggle => self.pick.toggle(&key),
            Gesture::Range => {
                let shown: Vec<String> = self.rail_shown.iter().map(Key::text).collect();
                self.pick.range(&key, &shown);
            }
        }
        if gesture != Gesture::Plain {
            // The rail holds the keys for the pick: Delete and Escape.
            window.focus(&self.rail_focus, cx);
        }
        cx.notify();
    }

    /// The rail's keys while it has focus: Escape empties the pick, Delete
    /// opens its menu armed.
    pub fn rail_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if e.keystroke.modifiers.modified() {
            return;
        }
        match e.keystroke.key.as_str() {
            "escape" if !self.pick.keys.is_empty() => self.pick.clear(),
            "delete" if !self.pick_targets().is_empty() => {
                // Where the pointer last was: by the rows just clicked.
                let at = window.mouse_position();
                self.menu_at = at;
                let rows = self
                    .pick_menu()
                    .into_iter()
                    .map(|i| i.row(&self.me))
                    .collect();
                self.host.menu_armed(rows, at, 0, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Turns tmux's mouse mode on or off on every tmux server of a device's
    /// node, only now that the operator asked (CHROTE never replays it).
    pub fn set_mouse(&mut self, node: &str, on: bool) {
        let device = self
            .where_is(node)
            .map(|p| p.device.to_uppercase())
            .unwrap_or_else(|| "the device".into());
        match self.fleet.mouse(node, on) {
            Some(id) => {
                self.mice.insert((node.to_string(), id), device);
            }
            None => self.report(
                &format!("Could not set the mouse on {device}: it is away"),
                true,
                None,
            ),
        }
    }

    /// A node answered a mouse change: on how many of its servers it took.
    pub fn moused(&mut self, node: &str, id: u32, ok: bool, text: &str) {
        let Some(device) = self.mice.remove(&(node.to_string(), id)) else {
            return;
        };
        let text = if text.is_empty() {
            "the node refused"
        } else {
            text
        };
        self.report(&format!("{device}: {text}"), !ok, None);
    }
}
