//! What every Parvi page is made of: the theme and widgets, the link to
//! the nodes, and the [`Host`] and [`Page`] that join a page to the app.

pub mod ask;
pub mod chords;
pub mod clock;
pub mod host;
pub mod hub;
pub mod layers;
pub mod link;
pub mod menu;
pub mod page;
pub mod settings;
pub mod text;
pub mod theme;
pub mod widgets;

pub use ask::{Answers, Ask};
pub use host::Host;
pub use hub::Hub;
pub use page::{Build, Mounted, Page, mount};
pub use parvi_core::Error;
