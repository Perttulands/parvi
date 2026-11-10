//! The parts a page of settings or help is made of: a scrolling page, its
//! sections, and a named line in one.

use crate::theme::{Theme, size};
use gpui::{Div, FontWeight, div, prelude::*, px};

pub fn section(t: &Theme, title: &str) -> Div {
    div()
        .w_full()
        .max_w(px(720.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .pb(px(18.))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text2)
                .child(title.to_uppercase()),
        )
}

pub fn line(t: &Theme, name: &str, value: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(px(size::CONTROL))
        .child(
            div()
                .w(px(size::LABEL_COL))
                .flex_none()
                .text_color(t.text2)
                .child(name.to_string()),
        )
        .child(value)
}

pub fn page(t: &Theme, id: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .size_full()
        .overflow_y_scroll()
        .bg(t.bg)
        .px(px(40.))
        .py(px(28.))
        .flex()
        .flex_col()
}
