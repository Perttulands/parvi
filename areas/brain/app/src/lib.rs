//! Brain page state and reader.
mod page;
mod prose;
pub mod state;
mod tokens;
mod view;

pub use page::BrainPage;

pub fn build(
    host: parvi_app_kit::Host,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> parvi_app_kit::Mounted {
    parvi_app_kit::mount(host, window, cx, BrainPage::new)
}
