//! The Files page's state.
pub mod model;
pub mod open;
pub mod store;

mod page;
mod view;
pub use page::FilesPage;

pub fn build(
    host: parvi_app_kit::Host,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> parvi_app_kit::Mounted {
    parvi_app_kit::mount(host, window, cx, FilesPage::new)
}
