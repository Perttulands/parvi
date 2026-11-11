//! Parvi's app shell: the window around the pages a binary lists. It knows
//! the pages only through `parvi_app_kit::Page`, so an area's tests drive it
//! with their own page without building any other area.

mod chrome;
mod close;
mod keys;
mod settings_page;
pub mod shell;
pub mod view_control;

pub use shell::Shell;

use gpui::{App, AppContext, Context, Window};
use parvi_app_kit::link::socket::Net;
use parvi_app_kit::theme::Theme;
use parvi_app_kit::widgets::field;
use parvi_app_kit::{Build, Hub};
use std::path::PathBuf;

/// What every window of the app needs before it opens.
pub fn prepare(cx: &mut App) {
    cx.set_global(Theme::violet());
    field::bind_keys(cx);
}

/// The shell in `window`, its pages made from `builds` and its link
/// dialling `node` (or the settings' node). Settings are kept at
/// `settings` (none: not kept). Refuses to start, naming them, when two
/// pages clash.
pub fn start(
    builds: &[Build],
    node: Option<String>,
    settings: Option<PathBuf>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Shell {
    let (tx, rx) = async_channel::unbounded();
    let hub = cx.new(|_| Hub::new(Net::start(), tx, settings));
    let weak = hub.downgrade();
    window
        .spawn(cx, async move |cx| {
            while let Ok(ev) = rx.recv().await {
                if weak.update(cx, |h, cx| h.handle(ev, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    follow_clicks(&hub, window, cx);
    let me = cx.weak_entity();
    window.on_window_should_close(cx, move |window, cx| {
        me.update(cx, |s, cx| s.may_close(window, cx))
            .unwrap_or(true)
    });
    let pages = shell::mount(builds, &hub, window, cx)
        .unwrap_or_else(|e| panic!("Parvi refuses to start: {e}"));
    let shell = Shell::new(hub.clone(), pages, window, cx);
    hub.update(cx, |h, cx| {
        if let Some(n) = node {
            h.settings.node = n;
        }
        h.connect(cx);
    });
    window.focus(&shell.focus, cx);
    shell
}

/// A click on a desktop notification brings this window forward and runs
/// what its page asked.
fn follow_clicks(hub: &gpui::Entity<Hub>, window: &Window, cx: &mut App) {
    let hub = hub.downgrade();
    let handle = window.window_handle();
    cx.on_system_notification_response(move |response, cx| {
        let Ok(Some(run)) = hub.update(cx, |h, _| h.clicked(&response.tag)) else {
            return;
        };
        let _ = handle.update(cx, |_, window, cx| {
            window.activate_window();
            run(window, cx);
        });
    });
}
