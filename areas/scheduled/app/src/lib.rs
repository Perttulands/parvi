//! The Scheduled page, CHROTE's (`ScheduledTasksView`, `ScheduleEditor`):
//! the toolbar with New task, the task cards, and beside them a task's
//! detail (Edit, Run now, Pause or Resume, Duplicate, Delete asked twice) or
//! the form (Prompt, Send to, When, Name). Each node keeps its own tasks and
//! sends them into its own sessions, so the cards stand in their device's
//! room and a task's sessions are all on one node.
//!
//! Read when shown, after a node's change event, once for a node that comes
//! into reach, and on Refresh (no timer reads the store);
//! countdowns tick every 30 s. Nothing ticks while the page is not shown.
//!
//! The sessions to send to come from the terminals page's published read model.

mod form;
mod page;
mod state;
mod tokens;
mod view;

use gpui::App;
use parvi_app_kit::{Mounted, mount};

pub use page::ScheduledPage;

/// The page's id in view control and settings, and the node area it needs.
pub const NAME: &str = "scheduled";

/// The actor the app writes as.
pub const ACTOR: &str = "user:parvi-app";

/// Makes the page; the app lists this in its `PAGES`.
pub fn build(host: parvi_app_kit::Host, window: &mut gpui::Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, ScheduledPage::new)
}
