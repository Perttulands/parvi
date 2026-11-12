//! What the page remembers on this device, in its section of the app's
//! settings: the workspace, the terminal's preferences, and the sessions
//! each node listed last.

use super::workspace::Workspace;
use crate::theme::size;
use parvi_app_kit::Setting;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub font_size: f32,
    /// Copy a selection to the clipboard as the mouse is released.
    pub copy_on_select: bool,
    pub sessions_open: bool,
    pub collapsed_groups: Vec<String>,
    pub recent_targets: Vec<String>,
    /// The Send drawer's width, as last dragged on this device.
    pub send_width: Option<f32>,
    pub last_harness: Option<String>,
    /// The folders launched in, the latest first, by node and user
    /// ([`launched_in`]).
    pub launched_folders: BTreeMap<String, Vec<String>>,
    /// Flags typed per harness, when they differ from the node's.
    pub flags: BTreeMap<String, String>,
    /// The launcher asks a harness to report when it finishes.
    pub notify: bool,
    /// A report marks its session's rail row and the tab holding it.
    pub event_marks: bool,
    /// A report pops the toast; the status line keeps it either way.
    pub event_toast: bool,
    /// A report plays its tone (`store::tones`).
    pub event_tone: bool,
    /// A report posts a desktop notification while the window is not active.
    pub event_desktop: bool,
    #[serde(deserialize_with = "mended")]
    pub workspace: Workspace,
    /// The sessions each node listed last, by node, so an away device keeps
    /// its windows and rail rows, named, until it answers again, across
    /// restarts too.
    pub kept: BTreeMap<String, Vec<Kept>>,
    /// The last known incarnation and restart recipe of each session.
    pub restarts: BTreeMap<String, Restart>,
}

/// What the Settings page lists for the terminals, each under its field's
/// name here.
pub const DECLARED: &[Setting] = &[
    Setting::number(
        "font_size",
        "Font size",
        size::TERM_DEFAULT as f64,
        (size::TERM_MIN as f64, size::TERM_MAX as f64, 1.),
    ),
    Setting::toggle("copy_on_select", "Copy on select", true)
        .note("A selection is copied as the mouse is released."),
    Setting::toggle("notify", "Report when done", true)
        .note("The launcher asks a harness to report when it finishes."),
    Setting::toggle("event_marks", "Mark reports", true)
        .note("A report marks its session's row and the tab holding it."),
    Setting::toggle("event_toast", "Toast reports", true),
    Setting::toggle("event_tone", "Play a tone", false),
    Setting::toggle("event_desktop", "Desktop notifications", true)
        .note("While the window is not active."),
];

/// A session as remembered: enough to list it while its device is away.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
}

/// What can be used to start an ended session again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Restart {
    pub id: String,
    pub folder: String,
    /// Absent for sessions whose launch the app did not witness.
    pub harness: Option<String>,
    pub flags: String,
    pub notify: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: size::TERM_DEFAULT,
            copy_on_select: true,
            sessions_open: true,
            collapsed_groups: vec![],
            recent_targets: vec![],
            send_width: None,
            last_harness: None,
            launched_folders: Default::default(),
            flags: Default::default(),
            notify: true,
            event_marks: true,
            event_toast: true,
            event_tone: false,
            event_desktop: true,
            workspace: Workspace::default(),
            kept: Default::default(),
            restarts: Default::default(),
        }
    }
}

/// A workspace that does not read is the default one, not a broken section.
fn mended<'de, D: Deserializer<'de>>(d: D) -> Result<Workspace, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(Workspace::parse(v).unwrap_or_default())
}

impl Settings {
    /// Takes what the Settings page sets ([`DECLARED`]) from `fresh`,
    /// keeping the rest as the page has it.
    pub fn take_declared(&mut self, fresh: Settings) {
        self.font_size = fresh.font_size;
        self.copy_on_select = fresh.copy_on_select;
        self.notify = fresh.notify;
        self.event_marks = fresh.event_marks;
        self.event_toast = fresh.event_toast;
        self.event_tone = fresh.event_tone;
        self.event_desktop = fresh.event_desktop;
    }

    /// The font size Ctrl and `key` set, as browsers and terminals do:
    /// `=` (or `+`) a point larger, `-` a point smaller, `0` the default.
    /// `None` for another key, or at the end of the range.
    pub fn font_for_key(&self, key: &str) -> Option<f32> {
        let to = match key {
            "=" | "+" => self.font_size + 1.,
            "-" => self.font_size - 1.,
            "0" => size::TERM_DEFAULT,
            _ => return None,
        }
        .clamp(size::TERM_MIN, size::TERM_MAX);
        (to != self.font_size).then_some(to)
    }

    /// Brings what was read within bounds.
    pub fn mend(mut self) -> Self {
        self.font_size = self.font_size.clamp(size::TERM_MIN, size::TERM_MAX);
        self
    }
}

/// Where [`Settings::launched_folders`] keeps a node's folders for `user`
/// (empty: the node's own).
pub fn launched_in(node: &str, user: &str) -> String {
    if user.is_empty() {
        node.to_string()
    } else {
        format!("{node}@{user}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_keys_step_the_font_within_its_range() {
        let at = |font_size| Settings {
            font_size,
            ..Settings::default()
        };
        assert_eq!(at(14.).font_for_key("="), Some(15.));
        assert_eq!(at(14.).font_for_key("+"), Some(15.));
        assert_eq!(at(14.).font_for_key("-"), Some(13.));
        assert_eq!(at(18.).font_for_key("0"), Some(size::TERM_DEFAULT));
        assert_eq!(at(size::TERM_MAX).font_for_key("="), None);
        assert_eq!(at(size::TERM_MIN).font_for_key("-"), None);
        assert_eq!(at(14.).font_for_key("x"), None);
    }

    fn parse(text: &str) -> Option<Settings> {
        serde_json::from_str::<Settings>(text)
            .ok()
            .map(Settings::mend)
    }

    /// The declarations name these fields, with their defaults, and a
    /// change to any of them is taken; nothing else is.
    #[test]
    fn what_the_settings_page_lists_is_these_fields() {
        let defaults = serde_json::to_value(Settings::default()).unwrap();
        let mut changed = defaults.clone();
        for setting in DECLARED {
            let kept = &defaults[setting.key];
            assert_eq!(*kept, setting.default_value(), "{}'s default", setting.key);
            changed[setting.key] = match kept {
                serde_json::Value::Bool(b) => (!b).into(),
                _ => setting.stepped(kept, 1.),
            };
        }
        changed["sessions_open"] = false.into();
        let fresh: Settings = serde_json::from_value(changed.clone()).unwrap();
        let mut mine = Settings::default();
        mine.take_declared(fresh);
        let taken = serde_json::to_value(&mine).unwrap();
        for setting in DECLARED {
            assert_eq!(taken[setting.key], changed[setting.key], "{}", setting.key);
        }
        assert_eq!(taken["sessions_open"], true, "not declared, not taken");
    }

    #[test]
    fn settings_round_trip_and_mend() {
        let mut s = Settings {
            font_size: 16.,
            ..Default::default()
        };
        s.workspace.add_window();
        let back = parse(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        let odd = parse(r#"{"font_size":40,"workspace":{"tabs":[]}}"#).unwrap();
        assert_eq!(odd.font_size, 20.);
        assert_eq!(odd.workspace, Workspace::default());
        // A device plays tones only once asked to, as CHROTE's.
        assert!(!odd.event_tone && odd.event_toast);
        // A settings file that is not an object does not read; the app's
        // settings, which hold this section, are where that is decided.
        assert!(parvi_app_kit::settings::Settings::parse("[1]").is_none());
    }
}
