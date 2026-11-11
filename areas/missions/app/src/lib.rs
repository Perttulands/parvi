//! Missions navigator and canvas.
mod page;
pub mod store;
use gpui::App;
use parvi_app_kit::{Mounted, mount};
pub fn build(host: parvi_app_kit::Host, window: &mut gpui::Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, page::MissionsPage::new)
}
