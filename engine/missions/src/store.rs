//! The mission state directory of a node:
//!
//! ```text
//! <state-dir>/missions/<slug>.mission.toml
//!             notes/<slug>.notes.toml
//!             layout/<slug>.layout.toml
//!             roles/<id>.toml and characters.json
//! ```
//!
//! Any of these directories and files may be a symlink, for example to a
//! mission kept in a repository. One file that cannot be read never hides the
//! rest: it is listed as broken, with the reason.

use parvi_mission_model::{Layout, Mission, MissionListing, Notes};
use std::path::{Path, PathBuf};

use crate::Error;
use crate::characters::{Character, read_characters};
use crate::links::{list_names, read_definition};
use crate::roles::RoleStore;

const MISSION_SUFFIX: &str = ".mission.toml";

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    /// The node's own mission state directory, under LOCALAPPDATA on Windows
    /// and XDG_DATA_HOME (or ~/.local/share) on Unix.
    pub fn default_root() -> Option<PathBuf> {
        let data = if cfg!(windows) {
            nonempty(std::env::var_os("LOCALAPPDATA"))?
        } else {
            nonempty(std::env::var_os("XDG_DATA_HOME"))
                .or_else(|| home_dir().map(|home| home.join(".local").join("share")))?
        };
        Some(data.join("parvi").join("missions"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn roles(&self) -> RoleStore {
        RoleStore::new(self.root.join("roles"))
    }

    pub fn characters(&self) -> Result<Vec<Character>, Error> {
        read_characters(self.roles().dir())
    }

    /// Every mission file by slug, the unreadable ones marked broken.
    pub fn list_missions(&self) -> Result<Vec<MissionListing>, Error> {
        let names = list_names(&self.root.join("missions"), MISSION_SUFFIX)?;
        Ok(names
            .into_iter()
            .map(|slug| match self.read_mission(&slug) {
                Ok(mission) => MissionListing {
                    id: mission.id,
                    title: mission.title,
                    rev: Some(mission.rev),
                    broken: None,
                    slug,
                },
                Err(err) => MissionListing {
                    slug,
                    id: String::new(),
                    title: String::new(),
                    rev: None,
                    broken: Some(if err.code == "broken_link" {
                        err.message
                    } else {
                        format!("{}: {}", err.code, err.message)
                    }),
                },
            })
            .collect())
    }

    pub fn read_mission(&self, slug: &str) -> Result<Mission, Error> {
        Mission::parse(&self.read_named("missions", slug, MISSION_SUFFIX)?)
    }

    /// The slug of the one mission whose slug or id is `selector`.
    pub fn resolve(&self, selector: &str) -> Result<String, Error> {
        check_slug(selector)?;
        let listings = self.list_missions()?;
        let matches: Vec<_> = (listings.iter())
            .filter(|listing| {
                listing.slug == selector || (!listing.id.is_empty() && listing.id == selector)
            })
            .collect();
        match matches.as_slice() {
            [listing] => Ok(listing.slug.clone()),
            [] => Err(Error::new("not_found", format!("mission {selector:?}"))),
            many => Err(Error::new(
                "ambiguous_selector",
                format!("mission {selector:?} matched {} missions", many.len()),
            )),
        }
    }

    /// A mission's notes; empty when it has none, or when its notes belong to
    /// a mission of another id.
    pub fn read_notes(&self, slug: &str) -> Result<Notes, Error> {
        let mission = self.read_mission(slug)?;
        match self.read_named("notes", slug, ".notes.toml") {
            Err(error) if error.code == "not_found" => Ok(Notes::empty(&mission.id)),
            Err(err) => Err(err),
            Ok(text) => {
                let notes = Notes::parse(&text)?;
                Ok(if notes.mission_id == mission.id {
                    notes
                } else {
                    Notes::empty(&mission.id)
                })
            }
        }
    }

    /// A mission's layout, or `None` when it has not been laid out.
    pub fn read_layout(&self, slug: &str) -> Result<Option<Layout>, Error> {
        match self.read_named("layout", slug, ".layout.toml") {
            Err(error) if error.code == "not_found" => Ok(None),
            Err(err) => Err(err),
            Ok(text) => Layout::parse(&text).map(Some),
        }
    }

    fn read_named(&self, dir: &str, slug: &str, suffix: &str) -> Result<String, Error> {
        check_slug(slug)?;
        read_definition(&self.root.join(dir).join(format!("{slug}{suffix}")))
    }
}

/// A slug names one file in its directory.
pub(crate) fn check_slug(slug: &str) -> Result<(), Error> {
    if slug.is_empty() || slug.contains(['/', '\\']) || slug.contains("..") || slug == "." {
        return Err(Error::new("invalid_name", format!("mission slug {slug:?}")));
    }
    Ok(())
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    (!home.is_empty()).then(|| PathBuf::from(home))
}

fn nonempty(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}
