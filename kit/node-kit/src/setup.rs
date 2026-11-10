//! What an area is built from.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use parvi_core::Error;

use crate::config::Config;
use crate::events::Log;
use crate::identity::Identity;
use crate::sessions::Sessions;

pub struct Setup {
    pub node: Identity,
    /// The node's config folder: its id, its address, and the files an
    /// area keeps beside them.
    pub dir: PathBuf,
    pub home: Option<PathBuf>,
    pub config: Config,
    /// Where the node serves.
    pub address: SocketAddr,
    pub events: Arc<Log>,
    /// The node's sessions, once the area that has them is built.
    pub sessions: Provided<dyn Sessions>,
}

/// Something one area provides and others use. Areas are built in the
/// binary's order and may hold on to this, reading it when they need it.
pub struct Provided<T: ?Sized> {
    what: &'static str,
    value: Arc<OnceLock<Arc<T>>>,
}

impl<T: ?Sized> Clone for Provided<T> {
    fn clone(&self) -> Self {
        Provided {
            what: self.what,
            value: self.value.clone(),
        }
    }
}

impl<T: ?Sized> Provided<T> {
    pub fn new(what: &'static str) -> Self {
        Provided {
            what,
            value: Arc::new(OnceLock::new()),
        }
    }

    /// Provides it; refused if another area already did.
    pub fn provide(&self, value: Arc<T>) -> Result<(), Error> {
        self.value
            .set(value)
            .map_err(|_| Error::new("provided_twice", format!("two areas provide {}", self.what)))
    }

    pub fn get(&self) -> Result<Arc<T>, Error> {
        self.value.get().cloned().ok_or_else(|| {
            Error::new(
                "not_provided",
                format!("no area on this node provides {}", self.what),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provided_once_and_read_by_whoever_holds_it() {
        let slot: Provided<str> = Provided::new("a greeting");
        let held = slot.clone();
        assert_eq!(held.get().unwrap_err().code, "not_provided");
        slot.provide(Arc::from("hello")).unwrap();
        assert_eq!(&*held.get().unwrap(), "hello");
        let twice = slot.provide(Arc::from("again")).unwrap_err();
        assert_eq!(twice.code, "provided_twice");
        assert!(twice.message.contains("a greeting"));
    }
}
