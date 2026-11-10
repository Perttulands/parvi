//! Local files and resumable copies within this node's configured roots.
mod access;
pub mod bridge;
pub mod diff;
pub mod disk;
pub mod drops;
pub mod error;
pub mod find;
mod identity;
mod receive;
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
    jobs: bridge::Jobs,
    receiving: Mutex<()>,
}

impl Files {
    pub fn new(node: String, roots: &[PathBuf], home: Option<&Path>) -> io::Result<Self> {
        Ok(Self {
            node,
            events: Arc::default(),
            roots: roots::Roots::new(roots, home)?,
            jobs: Mutex::default(),
            receiving: Mutex::default(),
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
    pub fn find(&self, path: &str, query: &str, content: bool) -> Result<Matches, FilesError> {
        find::search(&self.roots, path, query, content)
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
