//! What every Parvi crate shares, with no I/O: the one error shape, the ids
//! every resource carries, and the frame envelope of the node's socket.

mod error;
pub mod frame;
mod id;
pub mod json;

pub use error::Error;
pub use frame::Frame;
pub use id::{NodeId, Ref};
