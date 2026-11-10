//! The app's link to the nodes: one socket per node, through the serving
//! node, and plain HTTP to it.

pub mod client;
pub mod devices;
pub mod fleet;
pub mod frames;
pub mod http;
pub mod serving;
pub mod socket;
