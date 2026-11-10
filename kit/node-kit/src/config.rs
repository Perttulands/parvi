//! The node's config file, `config.toml` in its config dir: `[node]` for
//! the node itself and one section per area, which the area reads as its
//! own type ([`Config::section`]). `enabled = false` in an area's section
//! keeps the area from being built. An unknown section, an unknown key or a
//! value of the wrong kind stops the node, so a typo can't quietly drop a
//! setting. A relative path is read from the file's folder, and `~` is the
//! home folder ([`Config::path`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use parvi_core::Error;
use serde::Deserialize;
use serde::de::DeserializeOwned;

pub const FILE: &str = "config.toml";

/// The section the node itself reads.
pub const NODE: &str = "node";

const ENABLED: &str = "enabled";

#[derive(Clone, Debug)]
pub struct Config {
    table: toml::Table,
    /// The file's folder, where relative paths are.
    dir: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NodeSection {
    /// `addr:port`; the environment's port on loopback if absent.
    pub listen: Option<String>,
    /// The device's name; the hostname if absent.
    pub device: Option<String>,
    /// More host names requests may be addressed to.
    pub names: Vec<String>,
    /// Each environment's port: `windows = 7310`.
    pub ports: BTreeMap<String, u16>,
}

fn refused(message: String) -> Error {
    Error::new("bad_config", message)
}

impl Config {
    /// Reads `path`; no file is an empty one.
    pub fn load(path: &Path) -> Result<Config, Error> {
        let dir = path.parent().unwrap_or(Path::new(".")).to_owned();
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text, dir)
                .map_err(|e| refused(format!("in {}: {}", path.display(), e.message))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::parse("", dir)?),
            Err(e) => Err(refused(format!("reading {}: {e}", path.display()))),
        }
    }

    /// `text` as a file in `dir`.
    pub fn parse(text: &str, dir: PathBuf) -> Result<Config, Error> {
        let table: toml::Table = toml::from_str(text).map_err(|e| refused(e.to_string()))?;
        if let Some((name, _)) = table.iter().find(|(_, value)| !value.is_table()) {
            return Err(refused(format!("{name} is not a section")));
        }
        let config = Config { table, dir };
        config.node()?;
        Ok(config)
    }

    /// The node's own section.
    pub fn node(&self) -> Result<NodeSection, Error> {
        let node: NodeSection = self.section(NODE)?;
        for environment in node.ports.keys() {
            crate::identity::Ports::default()
                .set(&format!("{environment}=1"))
                .map_err(|e| refused(format!("node.ports: {e:#}")))?;
        }
        Ok(node)
    }

    /// The section `name`, but its `enabled`, read as `T`; an absent section
    /// is an empty one. `T` says which keys there are, and is best
    /// `#[serde(default, deny_unknown_fields)]`.
    pub fn section<T: DeserializeOwned>(&self, name: &str) -> Result<T, Error> {
        let mut table = match self.table.get(name) {
            Some(toml::Value::Table(table)) => table.clone(),
            _ => toml::Table::new(),
        };
        table.remove(ENABLED);
        T::deserialize(toml::Value::Table(table)).map_err(|e| refused(format!("[{name}]: {e}")))
    }

    /// Whether the section leaves its area on: unless it says
    /// `enabled = false`.
    pub fn enabled(&self, name: &str) -> Result<bool, Error> {
        match self.table.get(name).and_then(|s| s.get(ENABLED)) {
            None => Ok(true),
            Some(toml::Value::Boolean(on)) => Ok(*on),
            Some(_) => Err(refused(format!("[{name}] enabled is true or false"))),
        }
    }

    /// Refuses a section that is neither the node's nor one of `areas`.
    pub fn known(&self, areas: &[&str]) -> Result<(), Error> {
        match self
            .table
            .keys()
            .find(|name| *name != NODE && !areas.contains(&name.as_str()))
        {
            Some(name) => Err(refused(format!(
                "no section [{name}]; the sections are {NODE}, {}",
                areas.join(", ")
            ))),
            None => Ok(()),
        }
    }

    /// A path as the file means it: `~` the home folder, a relative path in
    /// the file's folder.
    pub fn path(&self, path: &Path) -> PathBuf {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        match (path.strip_prefix("~"), home) {
            (Ok(rest), Some(home)) => home.join(rest),
            _ if path.is_relative() => self.dir.join(path),
            _ => path.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, Deserialize)]
    #[serde(default, deny_unknown_fields)]
    struct Lights {
        bulb: Option<PathBuf>,
        rooms: Vec<PathBuf>,
        brightness: u8,
    }

    #[test]
    fn every_section_reads_and_relative_paths_live_beside_the_file() {
        let dir = std::env::temp_dir().join(format!("parvi-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        // An absolute folder on this platform, as a TOML literal string.
        let srv = if cfg!(windows) { r"C:\srv" } else { "/srv" };
        std::fs::write(
            &path,
            format!(
                r#"
[node]
listen = "127.0.0.1:7310"
ports = {{ windows = 7311 }}
[lights]
enabled = false
bulb = "bulb.conf"
rooms = ['{srv}', "notes", "~/notes"]
"#
            ),
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        let node = config.node().unwrap();
        assert_eq!(node.listen.as_deref(), Some("127.0.0.1:7310"));
        assert_eq!(node.ports["windows"], 7311);
        let lights: Lights = config.section("lights").unwrap();
        assert!(!config.enabled("lights").unwrap());
        assert!(config.enabled("absent").unwrap());
        assert_eq!(config.path(&lights.bulb.unwrap()), dir.join("bulb.conf"));
        let home = PathBuf::from(
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .unwrap(),
        );
        let rooms: Vec<PathBuf> = lights.rooms.iter().map(|p| config.path(p)).collect();
        assert_eq!(
            rooms,
            [PathBuf::from(srv), dir.join("notes"), home.join("notes")]
        );
        let absent = Config::load(&dir.join("absent.toml")).unwrap();
        assert_eq!(absent.node().unwrap(), NodeSection::default());
        assert_eq!(absent.section::<Lights>("lights").unwrap().brightness, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_typo_or_a_wrong_value_is_refused() {
        let parse = |text: &str| Config::parse(text, PathBuf::from("/"));
        assert!(parse("[node]\nlisen = \"x\"").is_err());
        assert!(parse("[node]\nports = { windows = \"x\" }").is_err());
        assert!(parse("[node]\nports = { mars = 1 }").is_err());
        assert!(parse("listen = 1").is_err());
        let config = parse("[nodes]\nlisten = \"x\"\n[lights]\nbrightnes = 3").unwrap();
        let error = config.known(&["lights"]).unwrap_err();
        assert_eq!(error.code, "bad_config");
        assert!(error.message.contains("[nodes]"), "{}", error.message);
        assert!(config.section::<Lights>("lights").is_err());
        let wrong = parse("[lights]\nenabled = \"no\"").unwrap();
        assert!(wrong.enabled("lights").is_err());
    }
}
