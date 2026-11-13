//! A view another page (or this one) opens beside the windows: a picture,
//! a file, a diagram. It opens as wide as its content asks, up to half the
//! room; its edge is dragged to size it and the width is kept; the button
//! on the edge gives it the whole room and back.

use super::super::page::Terminals;
use crate::store::beside;
use crate::theme::{size, theme};
use gpui::{AnyElement, App, Context, MouseButton, MouseMoveEvent, Window, div, prelude::*, px};
use parvi_app_kit::widgets::kit;

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

    /// Whether the view beside has the whole room.
    pub(super) fn beside_maximised(&self, cx: &App) -> bool {
        let shown = self.host.beside(cx).map(|v| v.entity_id());
        shown.is_some() && self.beside_max == shown
    }

    fn toggle_beside_max(&mut self, cx: &mut Context<Self>) {
        let shown = self.host.beside(cx).map(|v| v.entity_id());
        self.beside_max = if self.beside_max == shown {
            None
        } else {
            shown
        };
        cx.notify();
    }

    /// The view and its edge: `None` when nothing is beside.
    pub(super) fn render_beside(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, AnyElement)> {
        let view = self.host.beside(cx)?;
        let t = theme(cx).clone();
        let max = self.beside_maximised(cx);
        let width = self.beside_width(window, cx);
        let panel = div()
            .id("terminals-beside")
            .debug_selector(|| "terminals-beside".into())
            .h_full()
            .map(|d| {
                if max {
                    d.flex_1().min_w_0()
                } else {
                    d.w(px(width)).flex_none()
                }
            })
            .child(view)
            .into_any_element();
        let (word, tip) = if max {
            ("«", "Back beside the terminals")
        } else {
            ("»", "Give it the whole terminal area")
        };
        let seam = div()
            .id("beside-seam")
            .debug_selector(|| "beside-seam".into())
            .flex_none()
            .w(px(size::BESIDE_SEAM))
            .h_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .child(
                div()
                    .id("beside-max")
                    .debug_selector(|| "beside-max".into())
                    .flex_none()
                    .h(px(size::STRIP))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(t.dim)
                    .hover(|d| d.text_color(t.text))
                    .child(word)
                    .tooltip(kit::tip(tip))
                    .on_click(cx.listener(|s, _, _, cx| s.toggle_beside_max(cx))),
            )
            .when(!max, |d| {
                d.child(
                    div()
                        .id("beside-grip")
                        .debug_selector(|| "beside-grip".into())
                        .flex_1()
                        .cursor_col_resize()
                        .hover(|d| d.bg(t.accent_soft()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|s, _, _, cx| {
                                s.beside_resizing = true;
                                cx.stop_propagation();
                            }),
                        ),
                )
            })
            .into_any_element();
        Some((panel, seam))
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
