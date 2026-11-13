//! The terminals area's page: the Sessions rail, the windows of the
//! workspace, each a session's terminal, and what acts on sessions (the
//! launcher, Send, Overview, the scrollback search, Peek). Its state is its
//! own: the sessions and screens each node sends, and its settings section.
//! It answers Send to Session for every page, and asks the page that opens
//! paths to open one clicked in a terminal.

mod diagram;
mod harness;
mod http;
mod remote;
mod remotes;
mod sound;
pub mod store;
pub mod term;
pub mod theme;
mod ui;

use gpui::App;
use parvi_app_kit::{Host, Mounted, mount};

pub use ui::page::Terminals;

/// The page's id in view control and settings, and the node area it needs.
pub const NAME: &str = "terminals";

/// Makes the page; the app lists this in its `PAGES`. The terminal's
/// symbols font comes with it: box drawing and prompt glyphs the text
/// fonts lack (`term::paint`'s first fallback).
pub fn build(host: Host, window: &mut gpui::Window, cx: &mut App) -> Mounted {
    let symbols = include_bytes!("../assets/fonts/chrote-term-symbols.ttf");
    if let Err(e) = cx.text_system().add_fonts(vec![symbols.as_slice().into()]) {
        let why = format!("The terminal's symbols font did not load: {e}");
        host.report(parvi_app_kit::host::Status::failure(why), cx);
    }
    mount(host, window, cx, Terminals::new)
}
