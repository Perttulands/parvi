//! A figure's line: runs of line over a filled area, x by time (so a
//! silence shows as space), y to the line's top, a lone point as a stem.
//! Small in a list row; big in a figure's detail, with the time axis's
//! ticks as faint rules and a crosshair that follows the mouse.

use crate::readings::ReadingsPage;
use crate::tokens;
use gpui::{
    Bounds, Hsla, MouseMoveEvent, PathBuilder, Pixels, WeakEntity, canvas, fill, point, prelude::*,
    px, size,
};
use parvi_app_kit::widgets::boundary;

pub struct Line {
    pub runs: Vec<Vec<(u64, f32)>>,
    pub top: f32,
    pub start: u64,
    pub end: u64,
    /// The time axis's ticks, Unix ms.
    pub ticks: Vec<u64>,
    pub hover: Option<u64>,
    pub ink: Hsla,
    pub grid: Hsla,
}

/// The line, drawn to fill its parent. With `page`, the mouse over it
/// sets the moment the page reads.
pub fn chart(line: Line, page: Option<WeakEntity<ReadingsPage>>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        boundary::paint(move |b: Bounds<Pixels>, _: (), window, _| {
            draw(&line, b, window);
            if let Some(page) = &page {
                follow_mouse(line.start, line.end, b, page.clone(), window);
            }
        }),
    )
    .size_full()
}

fn x_of(at: u64, l: &Line, b: Bounds<Pixels>) -> Pixels {
    let span = (l.end - l.start).max(1) as f32;
    b.origin.x + b.size.width * ((at.saturating_sub(l.start)) as f32 / span).clamp(0., 1.)
}

fn y_of(v: f32, l: &Line, b: Bounds<Pixels>) -> Pixels {
    let share = (v / l.top).clamp(0., 1.);
    b.origin.y + b.size.height * (1. - share)
}

fn draw(l: &Line, b: Bounds<Pixels>, window: &mut gpui::Window) {
    let base = b.origin.y + b.size.height - px(tokens::HAIRLINE);
    window.paint_quad(fill(
        Bounds::new(
            point(b.origin.x, base),
            size(b.size.width, px(tokens::HAIRLINE)),
        ),
        l.grid,
    ));
    for at in &l.ticks {
        window.paint_quad(fill(
            Bounds::new(
                point(x_of(*at, l, b), b.origin.y),
                size(px(tokens::HAIRLINE), b.size.height),
            ),
            Hsla {
                a: l.grid.a * tokens::GRID_TICK,
                ..l.grid
            },
        ));
    }
    let area = Hsla {
        a: tokens::AREA_ALPHA,
        ..l.ink
    };
    for run in &l.runs {
        if let [(at, v)] = run.as_slice() {
            let x = x_of(*at, l, b);
            let y = y_of(*v, l, b);
            window.paint_quad(fill(
                Bounds::new(point(x, y), size(px(tokens::STROKE), base - y)),
                l.ink,
            ));
            continue;
        }
        let mut under = PathBuilder::fill();
        let mut stroke = PathBuilder::stroke(px(tokens::STROKE));
        for (i, (at, v)) in run.iter().enumerate() {
            let p = point(x_of(*at, l, b), y_of(*v, l, b));
            if i == 0 {
                under.move_to(point(p.x, base));
                under.line_to(p);
                stroke.move_to(p);
            } else {
                under.line_to(p);
                stroke.line_to(p);
            }
        }
        if let Some((at, _)) = run.last() {
            under.line_to(point(x_of(*at, l, b), base));
            under.close();
        }
        if let Ok(p) = under.build() {
            window.paint_path(p, area);
        }
        if let Ok(p) = stroke.build() {
            window.paint_path(p, l.ink);
        }
    }
    if let Some(at) = l.hover
        && at >= l.start
    {
        let x = x_of(at, l, b);
        window.paint_quad(fill(
            Bounds::new(
                point(x, b.origin.y),
                size(px(tokens::HAIRLINE), b.size.height),
            ),
            Hsla {
                a: tokens::LINE_ALPHA,
                ..l.ink
            },
        ));
    }
}

/// The mouse over the chart sets the moment the page reads; leaving it
/// clears it.
fn follow_mouse(
    start: u64,
    end: u64,
    b: Bounds<Pixels>,
    page: WeakEntity<ReadingsPage>,
    window: &mut gpui::Window,
) {
    window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
        if phase != gpui::DispatchPhase::Bubble {
            return;
        }
        let at = b.contains(&e.position).then(|| {
            let share = ((e.position.x - b.origin.x) / b.size.width).clamp(0., 1.);
            start + ((end - start) as f32 * share) as u64
        });
        let _ = page.update(cx, |s, cx| {
            if s.hover != at {
                s.hover = at;
                cx.notify();
            }
        });
    });
}
