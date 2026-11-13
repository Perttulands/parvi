//! A view another page (or this one) opens beside the windows: a picture,
//! a file, a diagram. It lies over the windows from the rail's edge, so no
//! session changes size for it; whatever it covers is simply covered. It
//! opens as wide as its content asks, up to half the room; its edge (a
//! grip) is dragged to size it and the width is kept; the view's Expand
//! gives it the whole area and back; Esc or Close takes it away.

use super::super::page::Terminals;
use crate::store::beside;
use crate::theme::{size, theme};
use gpui::{
    AnyElement, App, Context, KeyDownEvent, MouseButton, MouseMoveEvent, Window, div, prelude::*,
    px,
};

impl Terminals {
    /// The width the view beside takes, its edge left out.
    pub(super) fn beside_width(&self, window: &Window, cx: &App) -> f32 {
        let shown = self.host.beside(cx).map(|v| v.entity_id());
        let wish = self
            .beside_wish
            .filter(|(id, _)| Some(*id) == shown)
            .map(|(_, w)| w);
        beside::width(
            self.settings.beside_width,
            wish,
            self.workspace_room(window) - size::BESIDE_SEAM,
        )
    }

    /// The view beside with its edge, over the windows; `None` when nothing
    /// is beside.
    pub(super) fn render_beside(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Some(view) = self.host.beside(cx) else {
            self.beside_seen = None;
            return None;
        };
        // A view that just came takes the keyboard from the terminal, so
        // Esc reaches it rather than the program.
        if self.beside_seen != Some(view.entity_id()) {
            self.beside_seen = Some(view.entity_id());
            if self.term_focus.is_focused(window) {
                window.focus(&self.beside_focus, cx);
            }
        }
        let t = theme(cx).clone();
        let whole = self.host.whole_beside(crate::NAME, cx) == Some(true);
        let width = self.beside_width(window, cx);
        let grip = div()
            .id("beside-grip")
            .debug_selector(|| "beside-grip".into())
            .flex_none()
            .w(px(size::BESIDE_SEAM))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.bg)
            .border_l_1()
            .border_r_1()
            .border_color(t.divider)
            .cursor_col_resize()
            .hover(|d| d.bg(t.accent_soft()))
            // The grip's mark, seen at rest.
            .child(
                div()
                    .w(px(size::BESIDE_GRIP_W))
                    .h(px(size::BESIDE_GRIP_H))
                    .rounded(px(size::BESIDE_GRIP_W))
                    .bg(t.dim),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|s, _, _, cx| {
                    s.beside_resizing = true;
                    cx.stop_propagation();
                }),
            );
        Some(
            div()
                .id("terminals-beside")
                .debug_selector(|| "terminals-beside".into())
                .track_focus(&self.beside_focus)
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .map(|d| {
                    if whole {
                        d.right_0()
                    } else {
                        d.w(px(width + size::BESIDE_SEAM))
                    }
                })
                .flex()
                .shadow_lg()
                .on_key_down(cx.listener(|s, e: &KeyDownEvent, _, cx| {
                    if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                        s.host.close_beside(crate::NAME, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(div().flex_1().min_w_0().h_full().child(view))
                .when(!whole, |d| d.child(grip))
                .into_any_element(),
        )
    }

    /// While the edge is dragged: a layer over the page that follows the
    /// pointer and keeps the width when the button comes up.
    pub(super) fn render_beside_drag(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.beside_resizing {
            return None;
        }
        let left = if self.settings.sessions_open {
            size::RAIL
        } else {
            size::RAIL_SHUT
        };
        Some(
            div()
                .id("beside-drag")
                .absolute()
                .inset_0()
                .cursor_col_resize()
                .on_mouse_move(cx.listener(move |s, e: &MouseMoveEvent, window, cx| {
                    if e.pressed_button != Some(MouseButton::Left) {
                        s.beside_resizing = false;
                        s.save(cx);
                    } else {
                        let room = s.workspace_room(window) - size::BESIDE_SEAM;
                        let w = e.position.x / px(1.) - left - size::BESIDE_SEAM / 2.;
                        s.settings.beside_width = Some(beside::width(Some(w), None, room));
                    }
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|s, _, _, cx| {
                        s.beside_resizing = false;
                        s.save(cx);
                        cx.notify();
                    }),
                )
                .into_any_element(),
        )
    }
}
