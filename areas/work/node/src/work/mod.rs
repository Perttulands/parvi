//! Work: the work seam. Each configured project names its tracker, a Beads
//! store read with `bd` or a GitHub repository read with `gh api`, and one
//! interface serves the Work tab, Missions and the other modules
//! (ARCHITECTURE.md: Work; `docs/work.md`). Parvi files items through their configured trackers and never
//! handles a token.
//!
//! The public interface: [`Config`] says which projects; [`Work`] keeps
//! their snapshot current while watched, resolves a written reference to a
//! [`WorkRef`] and reads one [`Item`] with its family; each browser
//! connection holds a [`Channel`]; [`routes`] serves items over HTTP.

mod beads;
mod beads_parse;
mod channel;
mod config;
mod github;
mod github_item;
pub use parvi_work_contract::item;
mod lookup;
pub use parvi_work_contract::model;
mod parse;
mod poller;
mod routes;
mod tree;

pub use beads::Bd;
pub use channel::Channel;
pub use config::Config;
pub use github::Gh;
pub use poller::Work;
pub use routes::routes;
