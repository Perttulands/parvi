//! A mermaid diagram a session printed, drawn whole beside the terminals
//! as a picture is: Send hands it to a session, fit to the panel's width at
//! first, Fit / 1:1 / − / + as the picture viewer has them, and a drag (or
//! the wheel) moves it about.

use crate::store::zoom::{self, Level, Px};
use crate::theme::{size, theme};
use gpui::{
    Bounds, Context, FontWeight, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollEvent, Styled, Window,
    canvas, div, point, prelude::*, px,
};
use parvi_app_kit::host::Host;
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_app_kit::widgets::pane;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

/// A diagram painted: its pixels, its size at 1:1, and how many pixels a
/// 1:1 pixel has in them.
pub struct Painted {
    pub image: Arc<RenderImage>,
    pub natural: Px,
    pub density: f32,
}

pub struct DiagramView {
    host: Host,
    /// The mermaid text the session printed: what Send hands over.
    pub source: String,
    /// The SVG, painted again at the density the zoom wants.
    svg: Arc<str>,
    drawn: Result<Painted, String>,
    /// A paint at a new density is under way.
    painting: bool,
    level: Level,
    /// How far it was dragged, as last kept within its room.
    pan: Px,
    /// Where a drag began, and the pan then.
    held: Option<(Point<Pixels>, Px)>,
    room: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// Lines a wheel notch moves the diagram.
const WHEEL_LINE: f32 = 40.;
/// How far the density painted may be from the density wanted, as a
/// share, before it is painted again.
const DENSITY_SLACK: (f32, f32) = (0.8, 1.6);

impl Painted {
    pub fn of(d: crate::diagram::Drawn, density: f32) -> Self {
        Self {
            image: Arc::new(RenderImage::new(vec![image::Frame::new(d.pixels)])),
            natural: Px {
                w: d.width,
                h: d.height,
            },
            density,
        }
    }
}

impl DiagramView {
    pub fn new(
        host: Host,
        source: String,
        svg: Arc<str>,
        drawn: Result<Painted, String>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|v: &mut Self, cx| {
            if let Ok(p) = &v.drawn {
                cx.drop_image(p.image.clone(), None);
            }
        })
        .detach();
        Self {
            host,
            source,
            svg,
            drawn,
            painting: false,
            level: Level::Fit,
            pan: Px { w: 0., h: 0. },
            held: None,
            room: Rc::new(Cell::new(None)),
        }
    }

    /// Paints the diagram again with `density` pixels a 1:1 pixel, when
    /// what it has is far from that: sharp when zoomed in, no shimmer when
    /// drawn small.
    fn want_density(&mut self, density: f32, cx: &mut Context<Self>) {
        let Ok(p) = &self.drawn else {
            return;
        };
        let ratio = p.density / density;
        if self.painting || (DENSITY_SLACK.0..=DENSITY_SLACK.1).contains(&ratio) {
            return;
        }
        self.painting = true;
        let svg = self.svg.clone();
        let task = cx
            .background_executor()
            .spawn(async move { crate::diagram::paint(&svg, density) });
        cx.spawn(async move |this, cx| {
            let painted = task.await;
            let _ = this.update(cx, |v, cx| {
                v.painting = false;
                if let Ok(d) = painted {
                    let fresh = Painted::of(d, density);
                    if let Ok(old) = std::mem::replace(&mut v.drawn, Ok(fresh)) {
                        cx.drop_image(old.image, None);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The diagram as a session reads it: its mermaid block.
    fn reference(&self) -> String {
        format!("```mermaid\n{}\n```", self.source.trim_end())
    }

    fn room_px(&self) -> Option<Px> {
        self.room.get().map(|b| Px {
            w: f32::from(b.size.width),
            h: f32::from(b.size.height),
        })
    }

    fn set_level(&mut self, level: Level, cx: &mut Context<Self>) {
        self.level = level;
        cx.notify();
    }

    fn step(&mut self, up: bool, cx: &mut Context<Self>) {
        if let (Ok(p), Some(room)) = (&self.drawn, self.room_px()) {
            self.level = zoom::step(self.level, p.natural, room, up);
            cx.notify();
        }
    }

    fn pan_by(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        self.pan = Px {
            w: self.pan.w + dx,
            h: self.pan.h + dy,
        };
        cx.notify();
    }

    fn controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let t = theme(cx).clone();
        let percent = match (&self.drawn, self.room_px()) {
            (Ok(p), Some(room)) => format!("{}%", zoom::percent(self.level, p.natural, room)),
            _ => String::new(),
        };
        let word = |id: &'static str, label: &'static str, on: bool| {
            kit::button(&t, id, label, if on { Kind::Plain } else { Kind::Quiet })
                .debug_selector(move || id.into())
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                word("diagram-fit", "Fit", self.level == Level::Fit)
                    .on_click(cx.listener(|v, _, _, cx| v.set_level(Level::Fit, cx))),
            )
            .child(
                word("diagram-1", "1:1", self.level == Level::Percent(100))
                    .on_click(cx.listener(|v, _, _, cx| v.set_level(Level::Percent(100), cx))),
            )
            .child(
                word("diagram-out", "−", false)
                    .on_click(cx.listener(|v, _, _, cx| v.step(false, cx))),
            )
            .child(
                div()
                    .debug_selector(|| "diagram-percent".into())
                    .min_w(px(size::ZOOM_LABEL))
                    .flex()
                    .justify_center()
                    .text_size(px(size::SMALL))
                    .text_color(t.text2)
                    .child(percent),
            )
            .child(
                word("diagram-in", "+", false)
                    .on_click(cx.listener(|v, _, _, cx| v.step(true, cx))),
            )
    }
}

impl Render for DiagramView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let room = self.room.clone();
        let body = match &self.drawn {
            Ok(p) => {
                let (image, natural, level) = (p.image.clone(), p.natural, self.level);
                let pan = self.pan;
                let entity = cx.entity();
                canvas(
                    move |b, _, _| b,
                    move |_, b: Bounds<Pixels>, window, cx| {
                        let changed = room.get() != Some(b);
                        room.set(Some(b));
                        let r = Px {
                            w: f32::from(b.size.width),
                            h: f32::from(b.size.height),
                        };
                        let s = zoom::scale(level, natural, r);
                        let drawn = Px {
                            w: natural.w * s,
                            h: natural.h * s,
                        };
                        let (at, kept) = zoom::place(drawn, r, pan);
                        let to = Bounds::new(
                            point(b.left() + px(at.w), b.top() + px(at.h)),
                            gpui::size(px(drawn.w), px(drawn.h)),
                        );
                        window.with_content_mask(Some(gpui::ContentMask { bounds: b }), |w| {
                            let _ = w.paint_image(to, to, px(0.).into(), image.clone(), 0, false);
                        });
                        // The room or the pan as kept changed: draw again
                        // with them (the percent, a drag's start); and the
                        // pixels follow the zoom.
                        let density = (s * window.scale_factor())
                            .clamp(size::DIAGRAM_DENSITY.0, size::DIAGRAM_DENSITY.1);
                        let entity = entity.clone();
                        cx.defer(move |cx| {
                            entity.update(cx, |v, cx| {
                                if changed || kept != v.pan {
                                    v.pan = kept;
                                    cx.notify();
                                }
                                v.want_density(density, cx);
                            })
                        });
                    },
                )
                .size_full()
                .into_any_element()
            }
            Err(why) => kit::note(
                &t,
                format!("Mermaid could not draw it: {why}"),
                Some(t.error),
            )
            .p(px(size::DIAGRAM_PAD))
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
                    .pl(px(12.))
                    .pr(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(t.divider)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Diagram"),
                    )
                    .child(
                        pane::send(&t, "diagram-send", &self.host, self.reference())
                            .debug_selector(|| "diagram-send".into()),
                    )
                    .when(self.drawn.is_ok(), |d| d.child(self.controls(cx)))
                    .children(self.host.whole_beside(crate::NAME, cx).map(|whole| {
                        kit::whole_button(&t, whole).on_click(
                            cx.listener(|v, _, _, cx| v.host.toggle_whole_beside(crate::NAME, cx)),
                        )
                    }))
                    .child(
                        kit::button(&t, "diagram-close", "Close", Kind::Quiet).on_click(
                            cx.listener(|v, _, _, cx| v.host.close_beside(crate::NAME, cx)),
                        ),
                    ),
            )
            .child(
                div()
                    .id("diagram-room")
                    .debug_selector(|| "diagram-beside".into())
                    .flex_1()
                    .min_h_0()
                    .p(px(size::DIAGRAM_PAD))
                    .bg(t.term_bg)
                    .cursor_grab()
                    .when(self.held.is_some(), |d| d.cursor_grabbing())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|v, e: &MouseDownEvent, _, cx| {
                            v.held = Some((e.position, v.pan));
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, _, cx| {
                        let Some((from, pan)) = v.held else {
                            return;
                        };
                        if e.pressed_button != Some(MouseButton::Left) {
                            v.held = None;
                            cx.notify();
                            return;
                        }
                        v.pan = Px {
                            w: pan.w + f32::from(e.position.x - from.x),
                            h: pan.h + f32::from(e.position.y - from.y),
                        };
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|v, _, _, cx| {
                            v.held = None;
                            cx.notify();
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|v, _, _, cx| {
                            v.held = None;
                            cx.notify();
                        }),
                    )
                    .on_scroll(cx.listener(|v, e: &ScrollEvent, _, cx| {
                        let (dx, dy) = match e.delta {
                            ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
                            ScrollDelta::Lines(l) => (l.x * WHEEL_LINE, l.y * WHEEL_LINE),
                        };
                        // Shift turns the wheel sideways, for a mouse without a tilt.
                        if e.modifiers.shift && dx == 0. {
                            v.pan_by(dy, 0., cx);
                        } else {
                            v.pan_by(dx, dy, cx);
                        }
                    }))
                    .child(body),
            )
    }
}
