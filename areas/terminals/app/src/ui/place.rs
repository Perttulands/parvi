//! Where a terminal runs, drawn as two different kinds of thing: the device
//! is a solid plate in the device's colour, the environment a named strip
//! with its own texture. The one component for it; its values are tokens
//! in `theme::place`.

use crate::theme::place::{self, Texture};
use crate::theme::{Theme, alpha, rgb};
use gpui::{Bounds, Div, FontWeight, Hsla, PathBuilder, canvas, div, point, prelude::*, px};

/// The device plate: its name in capitals on its colour.
pub fn plate(device: &str) -> Div {
    div()
        .flex_none()
        .h(px(place::PLATE_H))
        .px(px(6.))
        .flex()
        .items_center()
        .rounded(px(2.))
        .bg(rgb(place::device_colour(device)))
        .text_color(rgb(0xffffff))
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .child(device.to_uppercase())
}

/// The environment strip: its word, then its texture.
pub fn strip(t: &Theme, environment: &str, width: f32) -> Div {
    let (word, texture) = place::environment(environment);
    let ink = alpha(t.text2, 0.55);
    div()
        .flex_none()
        .h(px(place::STRIP_H))
        .pl(px(6.))
        .pr(px(4.))
        .flex()
        .items_center()
        .gap(px(6.))
        .border_1()
        .border_color(alpha(t.text2, 0.35))
        .rounded(px(2.))
        .text_size(px(10.))
        .text_color(t.text2)
        .child(word)
        .child(
            div()
                .w(px(width))
                .h(px(10.))
                .child(texture_canvas(texture, ink)),
        )
}

/// A device and its environment side by side: the passport.
pub fn passport(t: &Theme, device: &str, environment: &str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .child(plate(device))
        .child(strip(t, environment, 28.))
}

fn texture_canvas(texture: Texture, ink: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        parvi_app_kit::widgets::boundary::paint(
            move |b: Bounds<gpui::Pixels>, _: (), window, _| {
                let (x0, y0) = (b.origin.x, b.origin.y);
                let (w, h) = (b.size.width, b.size.height);
                match texture {
                    Texture::Hatch => {
                        let mut x = px(0.);
                        while x < w {
                            let mut p = PathBuilder::stroke(px(1.));
                            p.move_to(point(x0 + x, y0 + h));
                            p.line_to(point(x0 + x + h * 0.6, y0));
                            if let Ok(path) = p.build() {
                                window.paint_path(path, ink);
                            }
                            x += px(4.);
                        }
                    }
                    Texture::Rules => {
                        for y in [h * 0.35, h * 0.65] {
                            window.paint_quad(gpui::fill(
                                Bounds::new(point(x0, y0 + y), gpui::size(w, px(1.))),
                                ink,
                            ));
                        }
                    }
                    Texture::Dots => {
                        let mut x = px(1.);
                        while x < w {
                            for y in [h * 0.3, h * 0.7] {
                                window.paint_quad(gpui::fill(
                                    Bounds::new(
                                        point(x0 + x, y0 + y),
                                        gpui::size(px(1.5), px(1.5)),
                                    ),
                                    ink,
                                ));
                            }
                            x += px(4.);
                        }
                    }
                }
            },
        ),
    )
    .size_full()
}
