//! What an area is built from, and the slots areas fill for one another.

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use parvi_core::Error;

use crate::config::Config;
use crate::events::Log;
use crate::identity::Identity;

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
    /// What areas offer one another: filled while areas are built, read
    /// once every area is ([`crate::Built::then`]).
    pub provided: Provided,
}

/// What areas provide one another, one value per interface: an area fills
/// a slot by the trait it offers (`provided.provide::<dyn Sessions>(..)`)
/// as it is built, and any area reads it by the same type once every area
/// is built, so the order of the binary's list never matters. The trait
/// lives with its provider (its contract or engine crate), not here.
#[derive(Clone, Default)]
pub struct Provided {
    slots: Arc<Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>>,
    /// Every area is built: slots are read, no longer filled.
    built: Arc<AtomicBool>,
}

impl Provided {
    /// Fills the slot for `T`; refused if another area did, or once every
    /// area is built.
    pub fn provide<T: ?Sized + Send + Sync + 'static>(&self, value: Arc<T>) -> Result<(), Error> {
        if self.built.load(Ordering::Acquire) {
            return Err(Error::new(
                "provided_late",
                format!(
                    "{} is provided after every area was built",
                    type_name::<T>()
                ),
            ));
        }
        let mut slots = self.slots.lock().unwrap();
        if slots.contains_key(&TypeId::of::<T>()) {
            return Err(Error::new(
                "provided_twice",
                format!("two areas provide {}", type_name::<T>()),
            ));
        }
        slots.insert(TypeId::of::<T>(), Box::new(value));
        Ok(())
    }

    /// What fills the slot for `T`. Read only once every area is built:
    /// an area that needs it finishes in [`crate::Built::then`].
    pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Arc<T>, Error> {
        if !self.built.load(Ordering::Acquire) {
            return Err(Error::new(
                "provided_early",
                format!(
                    "{} is read while areas are still being built: read it in Built::then",
                    type_name::<T>()
                ),
            ));
        }
        let slots = self.slots.lock().unwrap();
        slots
            .get(&TypeId::of::<T>())
            .and_then(|value| value.downcast_ref::<Arc<T>>())
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    "not_provided",
                    format!("no area on this node provides {}", type_name::<T>()),
                )
            })
    }

    /// Every area is built: from now on slots are read, not filled.
    pub(crate) fn seal(&self) {
        self.built.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    trait Greeting: Send + Sync {
        fn word(&self) -> &'static str;
    }

    struct Hello;

    impl Greeting for Hello {
        fn word(&self) -> &'static str {
            "hello"
        }
    }

    #[test]
    fn a_slot_is_filled_once_while_building_and_read_after() {
        let provided = Provided::default();
        let held = provided.clone();
        let early = held.get::<dyn Greeting>().err().unwrap();
        assert_eq!(early.code, "provided_early");
        provided.provide::<dyn Greeting>(Arc::new(Hello)).unwrap();
        let twice = provided
            .provide::<dyn Greeting>(Arc::new(Hello))
            .unwrap_err();
        assert_eq!(twice.code, "provided_twice");
        assert!(twice.message.contains("Greeting"), "{}", twice.message);
        provided.seal();
        assert_eq!(held.get::<dyn Greeting>().unwrap().word(), "hello");
        let missing = held.get::<str>().err().unwrap();
        assert_eq!(missing.code, "not_provided");
        let late = provided.provide::<str>(Arc::from("late")).unwrap_err();
        assert_eq!(late.code, "provided_late");
    }
}
