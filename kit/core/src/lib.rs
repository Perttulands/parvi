//! What every Parvi crate shares, with no I/O: the one error shape, the ids
//! every resource carries, and the frame envelope and hello of the node's
//! socket.

mod error;
pub mod frame;
mod hello;
mod id;
pub mod json;

pub use error::Error;
pub use frame::Frame;
pub use hello::{Hello, PROTOCOL};
pub use id::{NodeId, Ref};
