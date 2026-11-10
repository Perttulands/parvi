//! Layout presets, as CHROTE's: the windows of every tab saved under a
//! name, at most ten, and restored over the workspace. Tab names and how
//! many tabs show stay as they are; a restored session leaves any other
//! window it was bound to.

use super::workspace::{MAX_TABS, Tab, Workspace, mend as mend_tab};
use serde::{Deserialize, Serialize};

pub const MAX_PRESETS: usize = 10;
/// The longest name the menu takes.
pub const NAME_MAX: usize = 30;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    /// Every tab's windows and how many it shows, hidden tabs too.
    pub tabs: Vec<Tab>,
}

impl Workspace {
    /// Saves every tab's windows as a preset, under the name it returns.
    /// An empty name saves nothing.
    pub fn save_preset(&mut self, name: &str) -> Result<Option<String>, String> {
        let name: String = name.trim().chars().take(NAME_MAX).collect();
        if name.is_empty() {
            return Ok(None);
        }
        if self.presets.len() >= MAX_PRESETS {
            return Err(format!("Maximum {MAX_PRESETS} presets reached"));
        }
        let tabs = self
            .tabs
            .iter()
            .map(|t| Tab {
                name: None,
                ..t.clone()
            })
            .collect();
        self.presets.push(Preset {
            name: name.clone(),
            tabs,
        });
        Ok(Some(name))
    }

    /// Lays every tab out as preset `i` keeps it; its name, if there is one.
    pub fn restore_preset(&mut self, i: usize) -> Option<String> {
        let p = self.presets.get(i)?.clone();
        for (tab, kept) in self.tabs.iter_mut().zip(p.tabs) {
            tab.windows = kept.windows;
            tab.count = kept.count;
        }
        self.dedupe();
        self.focus = self.focus.min(self.windows().len() - 1);
        Some(p.name)
    }

    pub fn delete_preset(&mut self, i: usize) -> Option<String> {
        (i < self.presets.len()).then(|| self.presets.remove(i).name)
    }
}

/// Mends stored presets as the workspace is mended.
pub(super) fn mend(presets: &mut Vec<Preset>) {
    presets.truncate(MAX_PRESETS);
    for p in presets {
        p.tabs.truncate(MAX_TABS);
        p.tabs.resize_with(MAX_TABS, || Tab {
            name: None,
            windows: vec![],
            count: 1,
        });
        for t in &mut p.tabs {
            t.name = None;
            mend_tab(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sessions::Key;

    fn key(name: &str) -> Key {
        Key {
            node: "n".into(),
            user: String::new(),
            name: name.into(),
        }
    }

    #[test]
    fn a_preset_is_named_in_the_menu_and_restores_every_tab() {
        let mut ws = Workspace::default();
        ws.add_window();
        ws.bind(0, key("a"), None);
        ws.bind(1, key("b"), None);
        ws.rename_tab(0, "Ops");
        assert_eq!(ws.save_preset("  Focus  "), Ok(Some("Focus".into())));
        assert_eq!(
            ws.save_preset("   "),
            Ok(None),
            "an empty name saves nothing"
        );

        ws.clear(0);
        ws.set_windows(0, 1);
        ws.rename_tab(0, "Later");
        ws.add_tab();
        ws.bind(0, key("a"), None);
        assert_eq!(ws.restore_preset(0), Some("Focus".into()));
        assert_eq!(ws.tabs[0].count, 2);
        assert_eq!(ws.tabs[0].windows[0].shown().unwrap().key, key("a"));
        assert_eq!(ws.tabs[0].windows[1].shown().unwrap().key, key("b"));
        assert_eq!(ws.tabs[0].name.as_deref(), Some("Later"), "names stay");
        assert!(
            ws.tabs[1].windows[0].is_empty(),
            "every tab is laid out as it was saved"
        );
        assert_eq!(ws.count, 2, "how many tabs show stays");
        assert_eq!(ws.restore_preset(5), None);
    }

    #[test]
    fn ten_presets_is_the_most_and_one_can_be_deleted() {
        let mut ws = Workspace::default();
        for n in 0..MAX_PRESETS {
            ws.save_preset(&format!("p{n}")).unwrap();
        }
        assert_eq!(
            ws.save_preset("eleven"),
            Err("Maximum 10 presets reached".into())
        );
        assert_eq!(ws.delete_preset(3), Some("p3".into()));
        assert_eq!(ws.delete_preset(30), None);
        assert_eq!(ws.presets.len(), 9);
        let long = "x".repeat(40);
        assert_eq!(ws.save_preset(&long).unwrap().unwrap().len(), NAME_MAX);
    }

    #[test]
    fn stored_presets_are_mended() {
        let v = serde_json::json!({
            "tabs": [{"windows": [null]}],
            "active": 0, "focus": 0,
            "presets": [{"name": "old", "tabs": [{"name": "x", "windows": [{"node":"n","name":"a"}, null], "count": 9}]}]
        });
        let ws = Workspace::parse(v).unwrap();
        let p = &ws.presets[0];
        assert_eq!(p.tabs.len(), MAX_TABS);
        assert_eq!(p.tabs[0].count, 4);
        assert_eq!(p.tabs[0].windows.len(), 4);
        assert_eq!(p.tabs[0].name, None);
        assert_eq!(p.tabs[0].windows[0].shown().unwrap().key, key("a"));
    }
}
