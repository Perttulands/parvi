//! The terminal: one pane's grid, what keys and the mouse send to it, and
//! how it is painted. Nothing here knows about sessions or the workspace.

mod clusters;
pub mod drag;
pub mod keys;
pub mod links;
pub mod mouse;
pub mod paint;
pub mod screen;
