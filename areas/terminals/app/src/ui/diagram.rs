//! A mermaid diagram a session printed, drawn whole beside the terminals.

use crate::theme::{size, theme};
use gpui::{
    Context, FontWeight, Image, ImageFormat, IntoElement, ObjectFit, ParentElement, Render, Styled,
    StyledImage, Window, div, img, prelude::*, px,
};
use parvi_app_kit::host::Host;
use parvi_app_kit::widgets::kit::{self, Kind};
use std::sync::Arc;

pub struct DiagramView {
    host: Host,
    /// The SVG, or why there is none.
    drawn: Result<Arc<Image>, String>,
}

impl DiagramView {
    pub fn new(host: Host, svg: Result<String, String>) -> Self {
        Self {
            host,
            drawn: svg.map(|s| Arc::new(Image::from_bytes(ImageFormat::Svg, s.into_bytes()))),
        }
    }
}

impl Render for DiagramView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let body = match &self.drawn {
            Ok(image) => img(image.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            Err(why) => kit::note(
                &t,
                format!("Mermaid could not draw it: {why}"),
                Some(t.error),
            )
            .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_r_1()
            .border_color(t.divider)
            .child(
                div()
                    .h(px(size::STRIP))
                    .flex_none()
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(t.divider)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Diagram"),
                    )
                    .child(
                        kit::button(&t, "diagram-close", "Close", Kind::Quiet).on_click(
                            cx.listener(|v, _, _, cx| v.host.close_beside(crate::NAME, cx)),
                        ),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "diagram-beside".into())
                    .flex_1()
                    .min_h_0()
                    .p(px(size::DIAGRAM_PAD))
                    .bg(t.term_bg)
                    .child(body),
            )
    }
}
