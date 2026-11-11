//! The close guard: the window closes only when no guarded view holds
//! unsaved work, or when the operator chose to leave it (CHROTE's
//! leave-page guard). The shell owns the window's one close handler; pages
//! guard their views through `Host::guard_close`.

use crate::shell::Shell;
use gpui::{Context, FocusHandle, MouseButton, WeakEntity, Window, div, prelude::*, px};
use parvi_app_kit::Host;
use parvi_app_kit::theme::{board, size, theme};
use parvi_app_kit::widgets::kit::{self, Kind};

/// The guard's layer on the stack.
pub const LAYER: &str = "close";

/// What it asks: leave the unsaved work, or keep editing.
pub struct CloseGuard {
    shell: WeakEntity<Shell>,
    host: Host,
    /// What each guarded view holds unsaved.
    unsaved: Vec<String>,
    focus: FocusHandle,
}

impl CloseGuard {
    pub fn new(
        shell: WeakEntity<Shell>,
        host: Host,
        unsaved: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            shell,
            host,
            unsaved,
            focus: cx.focus_handle(),
        }
    }

    pub fn focus(&self) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CloseGuard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let title = match self.unsaved.as_slice() {
            [one] => format!("{one} is not saved."),
            all => format!("{} things are not saved.", all.len()),
        };
        let loses = format!("Closing Parvi now loses: {}.", self.unsaved.join(", "));
        let outside = self.host.clone();
        let stay = self.host.clone();
        let shell = self.shell.clone();
        div()
            .id("close-guard")
            .debug_selector(|| "close-guard".into())
            .track_focus(&self.focus)
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_center()
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                outside.close_layer(LAYER, cx)
            })
            .child(
                kit::panel(&t)
                    .w(px(size::NOTICE))
                    .p(px(board::GAP))
                    .flex()
                    .flex_col()
                    .gap(px(board::GAP))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().child(title))
                    .child(kit::note(&t, loses, None))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(board::GAP))
                            .child(
                                kit::button(&t, "close-leave", "Close without saving", Kind::Plain)
                                    .debug_selector(|| "close-leave".into())
                                    .on_click(move |_, window, cx| {
                                        let _ = shell.update(cx, |s, _| s.leave(window));
                                    }),
                            )
                            .child(
                                kit::button(&t, "close-stay", "Keep editing", Kind::Primary)
                                    .debug_selector(|| "close-stay".into())
                                    .on_click(move |_, _, cx| stay.close_layer(LAYER, cx)),
                            ),
                    ),
            )
    }
}
