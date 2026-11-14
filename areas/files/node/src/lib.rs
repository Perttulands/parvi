//! Local files and resumable copies within this node's configured roots.
mod access;
pub mod bridge;
pub mod diff;
pub mod disk;
pub mod drops;
pub mod error;
pub mod find;
mod identity;
mod index;
mod receive;
mod recent;
pub mod roots;
#[cfg(test)]
mod scratch;

pub use disk::version;
pub use error::FilesError;
pub use parvi_files_contract::{Listing, Matches, Progress, Root};
use parvi_node_kit::events::Log;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Files on one node. The area integration supplies its roots and event log.
pub struct Files {
    node: String,
    events: Arc<Log>,
    roots: roots::Roots,
    home: Option<PathBuf>,
    jobs: bridge::Jobs,
    receiving: Mutex<()>,
    index: Arc<index::Index>,
}

impl Files {
    pub fn new(node: String, roots: &[PathBuf], home: Option<&Path>) -> io::Result<Self> {
        Ok(Self {
            node,
            events: Arc::default(),
            roots: roots::Roots::new(roots, home)?,
            home: home.map(Path::to_path_buf),
            jobs: Mutex::default(),
            receiving: Mutex::default(),
            index: Arc::default(),
        })
    }

    pub fn with_events(mut self, events: Arc<Log>) -> Self {
        self.events = events;
        self
    }

    pub fn roots(&self) -> Vec<Root> {
        self.roots.list()
    }
    pub fn list(&self, path: &str) -> Result<Listing, FilesError> {
        disk::list(&self.roots, path)
    }
    pub fn read(&self, path: &str, limit: u64) -> Result<(Vec<u8>, String), FilesError> {
        let bytes = disk::read(&self.roots, path, limit)?;
        let version = disk::version(&bytes);
        Ok((bytes, version))
    }
    pub fn save(&self, path: &str, bytes: &[u8], base: Option<&str>) -> Result<(), FilesError> {
        disk::save(&self.roots, path, bytes, base)
    }
    pub fn create(&self, path: &str, bytes: &[u8]) -> Result<(), FilesError> {
        disk::upload(&self.roots, path, bytes)
    }
    pub fn create_folder(&self, path: &str) -> Result<(), FilesError> {
        disk::create_folder(&self.roots, path)
    }
    pub fn rename(&self, from: &str, to: &str) -> Result<(), FilesError> {
        disk::rename(&self.roots, from, to)
    }
    pub fn delete(&self, path: &str) -> Result<(), FilesError> {
        disk::delete(&self.roots, path)
    }
    /// Names (or, with `content`, text) under `path`.
    pub fn find(&self, path: &str, query: &str, content: bool) -> Result<Matches, FilesError> {
        find::search(&self.roots, path, query, content)
    }
    /// Files under every root by the words of `query` in their path. A
    /// query that is itself a path (absolute, or from `~`) answers the file
    /// it names, or the files it could go on to name. `near` is the folder
    /// the asker stands in: its files come first, and it is covered even
    /// where the roots' walk leaves it out. With `beside`, only its files
    /// are answered, at once: nothing waits for a walk of the roots.
    pub fn locate(&self, query: &str, near: Option<&str>, beside: bool) -> Matches {
        if let Some(named) = find::named(&self.roots, self.home.as_deref(), query) {
            return named;
        }
        let near = near
            .and_then(|folder| self.roots.existing(folder).ok())
            .filter(|folder| folder.is_dir());
        match (beside, near.as_deref()) {
            (true, Some(folder)) => self.index.find_beside(&self.roots, query, folder),
            (true, None) => Matches {
                matches: vec![],
                truncated: false,
            },
            (false, near) => self.index.find(&self.roots, query, near),
        }
    }
    /// The files changed most recently under the roots, or with a folder
    /// the files changed most recently under it, each as itself.
    pub fn recent(&self, under: Option<&str>) -> Result<parvi_files_contract::Recent, FilesError> {
        let files = match under {
            Some(folder) => {
                let folder = self.roots.existing(folder)?;
                if !folder.is_dir() {
                    return Err(FilesError::NotFolder);
                }
                self.index.recent_under(&self.roots, &folder)
            }
            None => self.index.recent(&self.roots),
        };
        Ok(parvi_files_contract::Recent { files })
    }
    pub fn stat(&self, paths: &[String]) -> Vec<Option<u64>> {
        disk::stat(&self.roots, paths)
    }
    pub fn diff(&self, path: &str) -> Result<parvi_files_contract::Diff, FilesError> {
        diff::read(&self.roots, path)
    }
    pub fn copy(
        self: &Arc<Self>,
        path: &str,
        target: &str,
        folder: &str,
    ) -> Result<String, FilesError> {
        let id = parvi_node_kit::identity::new_id();
        bridge::start(
            self.clone(),
            parvi_files_contract::TransferRequest {
                node: self.node.clone(),
                path: path.into(),
                target_node: target.into(),
                folder: folder.into(),
                id: id.clone(),
            },
        )?;
        Ok(id)
    }
    pub fn resume(self: &Arc<Self>, id: &str) -> Result<(), FilesError> {
        bridge::resume(self.clone(), id)
    }
    pub fn transfer(&self, id: &str) -> Option<Progress> {
        bridge::watch(self, id).map(|p| p.borrow().clone())
    }
}

mod commands;
mod routes;
pub use routes::{MAX_BYTES, routes};

use parvi_node_kit::{Area, Built, Listing as AreaListing, Setup};
use serde::Deserialize;

pub const NAME: &str = "files";
pub const LISTING: AreaListing = AreaListing {
    name: NAME,
    kinds: &[],
    commands: &[
        "file_list",
        "file_read",
        "file_diff",
        "file_recent",
        "file_download",
        "file_upload",
        "file_write",
        "file_mkdir",
        "file_move",
        "file_delete",
        "file_find",
        "file_locate",
        "file_search",
        "file_copy",
        "file_resume",
        "file_transfer",
    ],
    build,
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    roots: Option<Vec<PathBuf>>,
    drops: Option<PathBuf>,
}

struct FilesArea {
    files: Arc<Files>,
    drops: Arc<drops::Drops>,
    home: Option<PathBuf>,
}

pub fn build(setup: &Setup) -> anyhow::Result<Built> {
    let section: Section = setup.config.section(NAME)?;
    let roots: Vec<PathBuf> = match section.roots {
        Some(roots) => roots.iter().map(|p| setup.config.path(p)).collect(),
        None => setup.home.iter().cloned().collect(),
    };
    let files = Files::new(setup.node.id.to_string(), &roots, setup.home.as_deref())?
        .with_events(setup.events.clone());
    let dir = section
        .drops
        .map(|p| setup.config.path(&p))
        .unwrap_or_else(|| setup.dir.join("drops"));
    let home = setup.home.clone();
    Built::then(move |provided| {
        let users = provided
            .get::<dyn parvi_term::Sessions>()
            .map(|sessions| sessions.users())
            .unwrap_or_default();
        Ok(Arc::new(FilesArea {
            files: Arc::new(files),
            drops: Arc::new(drops::Drops::new(dir, users)),
            home,
        }) as Arc<dyn Area>)
    })
}

impl Area for FilesArea {
    fn routes(&self) -> axum::Router {
        routes::routes(self.files.clone(), self.drops.clone())
    }
    fn commands(&self) -> Vec<parvi_node_kit::Command> {
        commands::commands(self.files.clone(), self.home.clone())
    }
}
