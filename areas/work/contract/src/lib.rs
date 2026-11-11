//! Shared Work wire types and channel constants.

pub mod item;
pub mod model;
mod resolver;
pub use item::{Family, Item};
pub use model::*;
/// The syntax of a work item's reference, for pages that recognise one in text.
pub use parvi_work_ref as work_ref;
pub use resolver::{Resolver, Resolving};
pub const WATCH: u8 = 0x98;
pub const SNAPSHOT: u8 = 0x90;
pub const KINDS: parvi_core::frame::Kinds = &[WATCH..=WATCH];
pub const BAD_REQUEST: &str = "bad_request";
pub const TRACKER_ERROR: &str = "tracker_error";
