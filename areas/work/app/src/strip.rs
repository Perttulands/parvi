//! The strip of days above the table (CHROTE's `TimeStrip`): for each day
//! the work open at its end, with what was created and closed; a drag
//! across it keeps the items created, updated or closed in those days.

use crate::Work;
use crate::store::work::days::{RECENT_DAYS, day_label, strip_start};
use crate::tokens::*;
use gpui::{Context, Div, MouseButton, div, prelude::*, px};
use parvi_app_kit::theme::{alpha, size, theme};
use parvi_app_kit::widgets::kit::{self, Kind};

impl Work {
    pub(super) fn strip(&self, cx: &mut Context<Self>) -> Div {
        let t = theme(cx).clone();
        let d = &self.board.days;
        let s = &self.sliced;
        let days = s.open.len();
        let start = strip_start(
            days,
            self.slice.all_days,
            self.slice.range.map(|r| r.0),
            d.first,
        );
        let top = s.open[start..].iter().copied().max().unwrap_or(0).max(1) as f32;
        let mut bars = div()
            .id("work-strip")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_end()
            .justify_end()
            .gap(px(SPACE_HAIRLINE))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|w, _, _, cx| {
                    w.drag_from = None;
                    cx.notify();
                }),
            );
        for at in start..days {
            let day = d.first + at as i32;
            let kept = self
                .slice
                .range
                .is_none_or(|(from, to)| (from..=to).contains(&day));
            let h = (STRIP_H - STRIP_BAR_TOP_GAP) * s.open[at] as f32 / top;
            let tip = format!(
                "{}: {} open, {} created, {} closed",
                day_label(day),
                s.open[at],
                s.created[at],
                s.closed[at]
            );
            bars = bars.child(
                div()
                    .id(("wday", at))
                    .flex_1()
                    .max_w(px(STRIP_BAR_MAX_W))
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .cursor_pointer()
                    .tooltip(kit::tip(&tip))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |w, _, _, cx| {
                            w.drag_from = None;
                            w.drag_to(day, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(move |w, e: &gpui::MouseMoveEvent, _, cx| {
                        if w.drag_from.is_some() && e.pressed_button == Some(MouseButton::Left) {
                            w.drag_to(day, cx);
                        }
                    }))
                    .child(
                        div()
                            .h(px(h.max(STRIP_BAR_MIN_H)))
                            .bg(if kept {
                                alpha(t.accent, STRIP_BAR_FILL)
                            } else {
                                t.divider
                            })
                            .when(s.created[at] > 0, |b| {
                                b.border_t_2()
                                    .border_color(if kept { t.accent } else { t.text2 })
                            }),
                    ),
            );
        }
        let label = match self.slice.range {
            None => "All days".to_string(),
            Some((from, to)) if from == to => day_label(from),
            Some((from, to)) => format!("{} to {}", day_label(from), day_label(to)),
        };
        let span = if self.slice.all_days {
            format!("Last {RECENT_DAYS} days")
        } else {
            "Every day".to_string()
        };
        div()
            .flex_none()
            .h(px(STRIP_H))
            .px(px(SPACE_4XL))
            .py(px(SPACE_XS))
            .flex()
            .items_center()
            .gap(px(SPACE_XL))
            .border_b_1()
            .border_color(t.divider)
            .child(
                div()
                    .w(px(STRIP_LABEL_W))
                    .flex_none()
                    .text_size(px(size::SMALL))
                    .text_color(if self.slice.range.is_some() {
                        t.accent
                    } else {
                        t.text2
                    })
                    .child(label),
            )
            .when(self.slice.range.is_some(), |r| {
                r.child(
                    kit::button(&t, "w-range-clear", "×", Kind::Quiet)
                        .tooltip(kit::tip("Clear the date range"))
                        .on_click(cx.listener(|w, _, _, cx| w.clear_range(cx))),
                )
            })
            .child(bars)
            .when(days > RECENT_DAYS, |r| {
                r.child(
                    kit::button(&t, "w-span", &span, Kind::Quiet)
                        .text_size(px(size::SMALL))
                        .on_click(cx.listener(|w, _, _, cx| {
                            w.slice.all_days = !w.slice.all_days;
                            cx.notify();
                        })),
                )
            })
    }
}
