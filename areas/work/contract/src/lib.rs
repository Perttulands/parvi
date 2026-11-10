//! Shared Work wire types and channel constants.

pub mod item;
pub mod model;
pub use item::{Family, Item};
pub use model::*;
pub const WATCH: u8 = 0x98;
pub const SNAPSHOT: u8 = 0x90;
pub const KINDS: parvi_core::frame::Kinds = &[WATCH..=WATCH];
pub const BAD_REQUEST: &str = "bad_request";
pub const TRACKER_ERROR: &str = "tracker_error";
