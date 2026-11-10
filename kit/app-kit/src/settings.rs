//! What the app remembers on this device: the node it talks to, whether
//! keys are on, the fleet as last seen, and each page's own section, by
//! page id. One JSON file in the config folder (`%APPDATA%\Parvi\app.json`,
//! `$XDG_CONFIG_HOME/parvi/app.json`), or the file `PARVI_APP_CONFIG` names.
//! A file that doesn't read is reported and set aside, never silently
//! replaced; what it holds that this app does not know is kept as it reads.

use parvi_core::Error;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The node of the app's own environment, on its default port: a Windows
/// node listens on 7301 so it can sit beside a WSL node on 7300.
#[cfg(not(windows))]
pub const DEFAULT_NODE: &str = "http://127.0.0.1:7300";
#[cfg(windows)]
pub const DEFAULT_NODE: &str = "http://127.0.0.1:7301";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub node: String,
    /// Keys on: chords and the leader are intercepted.
    pub keys: bool,
    /// The fleet as last seen: each node's device and environment, so an
    /// away device keeps its name until it answers again, across restarts
    /// too, and an address to serve through while the configured node is
    /// away.
    pub places: BTreeMap<String, Place>,
    /// Each page's section, by its id, and whatever else the file holds.
    #[serde(flatten)]
    pub sections: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub device: String,
    pub environment: String,
    /// When its link last went, Unix ms: an away device says since when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<u64>,
    /// Where its serving node last heard it on the tailnet: where the app
    /// dials it when the configured node goes away.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub address: String,
    /// What a page remembers of it, kept as it reads.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            node: DEFAULT_NODE.into(),
            keys: true,
            places: Default::default(),
            sections: Map::new(),
        }
    }
}

pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PARVI_APP_CONFIG") {
        return Some(p.into());
    }
    let dir = if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Parvi"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|d| d.join("parvi"))
    }?;
    Some(dir.join("app.json"))
}

impl Settings {
    /// Reads the file; a missing one is the defaults. A broken one is moved
    /// to `app.json.broken` and named in the returned notice.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => {
                return (
                    Self::default(),
                    Some(format!("Settings could not be read: {e}")),
                );
            }
        };
        match Self::parse(&text) {
            Some(s) => (s, None),
            None => {
                let aside = path.with_extension("json.broken");
                let _ = std::fs::rename(path, &aside);
                (
                    Self::default(),
                    Some(format!(
                        "Settings did not read; kept as {}",
                        aside.display()
                    )),
                )
            }
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    /// Page `id`'s section: its defaults while it has none.
    pub fn section<T: DeserializeOwned + Default>(&self, id: &str) -> Result<T, Error> {
        match self.sections.get(id) {
            None => Ok(T::default()),
            Some(v) => T::deserialize(v).map_err(|e| {
                Error::new(
                    "settings_unreadable",
                    format!("{id}'s settings do not read: {e}"),
                )
            }),
        }
    }

    /// Writes page `id`'s section.
    pub fn set_section<T: Serialize>(&mut self, id: &str, value: &T) -> Result<(), Error> {
        let v = serde_json::to_value(value).map_err(|e| {
            Error::new(
                "settings_unwritable",
                format!("{id}'s settings do not write: {e}"),
            )
        })?;
        self.sections.insert(id.to_string(), v);
        Ok(())
    }

    /// What page `id`'s section keeps under `key`.
    pub fn value(&self, id: &str, key: &str) -> Option<&Value> {
        self.sections.get(id)?.get(key)
    }

    /// Keeps `value` under `key` in page `id`'s section, leaving the rest
    /// of it as it is.
    pub fn set_value(&mut self, id: &str, key: &str, value: Value) {
        let section = self
            .sections
            .entry(id.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !section.is_object() {
            *section = Value::Object(Map::new());
        }
        if let Value::Object(section) = section {
            section.insert(key.to_string(), value);
        }
    }

    /// Writes through a temporary file, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_keeping_what_they_do_not_know() {
        let mut s = Settings {
            keys: false,
            ..Default::default()
        };
        s.places.insert(
            "n".into(),
            Place {
                device: "rig".into(),
                environment: "wsl".into(),
                ..Default::default()
            },
        );
        s.set_section("toy", &vec![1, 2]).unwrap();
        let back = Settings::parse(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.section::<Vec<u8>>("toy").unwrap(), [1, 2]);
        assert_eq!(back.section::<Vec<u8>>("other").unwrap(), Vec::<u8>::new());
        assert_eq!(
            back.section::<String>("toy").unwrap_err().code,
            "settings_unreadable"
        );
        // What an older app wrote, and a page not built here, stays.
        let old = r#"{"node":"http://rig:7300","font_size":16,"places":{"n":{"device":"rig","environment":"wsl","sessions":[{"name":"a"}]}}}"#;
        let s = Settings::parse(old).unwrap();
        assert!(s.keys, "keys default on");
        let again: Value = serde_json::to_value(&s).unwrap();
        assert_eq!(again["font_size"], 16);
        assert_eq!(again["places"]["n"]["sessions"][0]["name"], "a");
        assert!(Settings::parse("[1]").is_none());
    }

    #[test]
    fn a_broken_file_is_set_aside_not_lost() {
        let dir = std::env::temp_dir().join(format!("parvi-app-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("app.json");
        std::fs::write(&p, "{not json").unwrap();
        let (s, notice) = Settings::load(&p);
        assert_eq!(s, Settings::default());
        assert!(notice.unwrap().contains("app.json.broken"));
        assert!(dir.join("app.json.broken").exists());
        Settings::default().save(&p).unwrap();
        assert_eq!(Settings::load(&p).0, Settings::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
