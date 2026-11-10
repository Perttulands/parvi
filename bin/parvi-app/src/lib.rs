//! Parvi's native app: the shell around the pages the app registers. The
//! pages come from `main.rs`'s `PAGES`; the shell knows them only through
//! `parvi_app_kit::Page`.

pub mod app_log;
mod chrome;
mod keys;
pub mod shell;
pub mod view_control;

pub use shell::Shell;

use gpui::{
    App, AppContext, Bounds, Context, Window, WindowBounds, WindowOptions, point, px, size,
};
use parvi_app_kit::link::socket::Net;
use parvi_app_kit::theme::Theme;
use parvi_app_kit::widgets::field;
use parvi_app_kit::{Build, Hub};
use std::borrow::Cow;
use std::path::PathBuf;

pub fn fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf").as_slice()),
    ]
}

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

/// `PARVI_APP_BOUNDS=x,y,w,h` places the window exactly (tests on an
/// invisible display); otherwise it opens centred.
fn bounds(cx: &App) -> Bounds<gpui::Pixels> {
    let given: Option<Vec<f32>> = std::env::var("PARVI_APP_BOUNDS")
        .ok()
        .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect());
    match given.as_deref() {
        Some([x, y, w, h]) => Bounds::new(point(px(*x), px(*y)), size(px(*w), px(*h))),
        _ => Bounds::centered(None, size(px(1440.), px(900.)), cx),
    }
}

/// The app: `parvi-app [--node http://host:port]`.
pub fn run(builds: &'static [Build]) {
    let node = {
        let args: Vec<String> = std::env::args().collect();
        args.iter()
            .position(|a| a == "--node")
            .and_then(|i| args.get(i + 1).cloned())
    };
    let failed_notices = app_log::install();
    gpui_platform::application().run(move |cx: &mut App| {
        if let Err(e) = cx.text_system().add_fonts(fonts()) {
            panic!("the bundled fonts did not load: {e}");
        }
        prepare(cx);
        // Desktop notifications need the app named before they are posted.
        cx.set_app_identity("io.parvi.app", "Parvi");
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds(cx))),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Parvi".into()),
                    ..Default::default()
                }),
                app_id: Some("parvi".into()),
                ..Default::default()
            },
            |window, cx| {
                let shell =
                    cx.new(|cx| start(builds, node, parvi_app_kit::settings::path(), window, cx));
                let hub = shell.read(cx).hub().downgrade();
                window
                    .spawn(cx, async move |cx| {
                        while let Ok(why) = failed_notices.recv().await {
                            let status = parvi_app_kit::host::Status::failure(format!(
                                "A desktop notification could not be shown: {why}"
                            ));
                            if hub.update(cx, |h, cx| h.report(status, cx)).is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                shell
            },
        );
        if let Err(e) = opened {
            panic!("the window did not open: {e}");
        }
        cx.on_window_closed(|cx, _| cx.quit()).detach();
    });
}
