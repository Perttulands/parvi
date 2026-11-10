//! The app's real list of pages mounts together: no two share an id, a
//! frame kind, an `everywhere` chord or a setting key, and none takes a kind
//! the link or the shell owns. The hub never dials a node.

use gpui::{AppContext, TestAppContext};
use parvi_app_kit::Hub;
use parvi_app_kit::link::socket::Net;
use parvi_app_shell::shell::mount;

#[gpui::test]
fn the_listed_pages_mount_together(cx: &mut TestAppContext) {
    cx.update(parvi_app_shell::prepare);
    let cx = cx.add_empty_window();
    let (tx, _rx) = async_channel::unbounded();
    let mounted = cx.update(|window, cx| {
        let hub = cx.new(|_| Hub::new(Net::start(), tx, None));
        mount(parvi_app::PAGES, &hub, window, cx).map(|pages| pages.len())
    });
    // Each listed page, and the shell's Settings page.
    assert_eq!(mounted.unwrap(), parvi_app::PAGES.len() + 1);
}
