//! Pictures a session's output names: a small one at the right end of the
//! row that names it, where the row has room; a bigger one while the
//! pointer rests on the name or the small one; the whole picture in Files
//! beside on a click, as any path opens. The grid's text never moves.

use super::super::page::Terminals;
use crate::http;
use crate::store::pictures::{self, Key};
use crate::term::links::{self, Link, Target};
use crate::term::paint::Metrics;
use crate::term::screen::Screen;
use crate::theme::{Theme, size};
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use gpui::{
    App, BorderStyle, Bounds, Context, Entity, Pixels, RenderImage, Window, point, px, quad,
};
use parvi_app_kit::host::Request;
use parvi_core::NodeId;
use std::sync::Arc;

/// The most pictures one window draws small at a time.
const MOST: usize = 12;

/// A small picture drawn in a window's grid, standing for its link.
#[derive(Clone)]
pub struct Chip {
    pub bounds: Bounds<Pixels>,
    pub link: Link,
}

/// Where a window's pictures are read: its session's device and folder.
#[derive(Clone)]
pub struct Place {
    pub node: String,
    pub cwd: Option<String>,
}

impl Place {
    fn key(&self, path: &str) -> Key {
        (self.node.clone(), links::resolve(self.cwd.as_deref(), path))
    }
}

/// The grid as painted: its screen's top-left, cells, the part of the
/// window it shows in, and the cells of the link the pointer is on.
pub struct Painted<'a> {
    pub origin: gpui::Point<Pixels>,
    pub m: Metrics,
    pub view: Bounds<Pixels>,
    pub hover: &'a [GridPoint],
}

/// Paints the pictures named on the rows in view, and the bigger one of
/// the link in `hover`. Returns the small ones drawn, for the pointer.
pub fn paint(
    page: &Entity<Terminals>,
    place: &Place,
    s: &Screen,
    at: Painted,
    t: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> Vec<Chip> {
    let (origin, m, view, hover) = (at.origin, at.m, at.view, at.hover);
    let h = m.cell.height;
    let offset = s.term.grid().display_offset() as i32;
    // Rows wholly in view.
    let first = ((view.top() - origin.y) / h).ceil().max(0.) as i32;
    let last = (((view.bottom() - origin.y) / h).floor() as i32).min(s.rows as i32) - 1;
    if last < first {
        return Vec::new();
    }
    let named = links::in_lines(&s.term, Line(first - offset), Line(last - offset))
        .into_iter()
        .filter_map(|l| match &l.target {
            Target::Path(p) if pictures::format(p).is_some() => Some((place.key(p), l)),
            _ => None,
        })
        .take(MOST);
    let mut chips: Vec<Chip> = Vec::new();
    let mut preview = None;
    let top = |row: i32| origin.y + h * row as f32 + px(size::PICTURE_CHIP_INSET);
    for (key, link) in named {
        let Some(image) = page.update(cx, |p, cx| p.picture(&key, cx)) else {
            continue;
        };
        if hover.first().is_some_and(|c| link.cells.contains(c)) {
            preview = Some((image.clone(), link.clone()));
        }
        let Some(end) = link.cells.last() else {
            continue;
        };
        let row = end.line.0 + offset;
        if row < first || row > last {
            continue;
        }
        let ch = h - px(2. * size::PICTURE_CHIP_INSET);
        let cw = ch * size::PICTURE_CHIP_RATIO;
        // Pictures named on one row line up leftward from its end.
        let edge = origin.x + m.cell.width * s.cols as f32;
        let right = chips
            .iter()
            .filter(|c: &&Chip| c.bounds.top() == top(row))
            .map(|c| c.bounds.left())
            .fold(edge, Pixels::min)
            - px(size::PICTURE_CHIP_INSET);
        let left = right - cw;
        // Only over blank cells: the text stays readable.
        let from = ((left - origin.x) / m.cell.width).floor().max(0.) as usize;
        let grid = s.term.grid();
        let blank = (from..s.cols)
            .all(|c| matches!(grid[GridPoint::new(end.line, Column(c))].c, ' ' | '\0'));
        if !blank || end.column.0 >= from {
            continue;
        }
        let bounds = Bounds::new(point(left, top(row)), gpui::size(cw, ch));
        let _ = window.paint_image(
            bounds,
            cover(&image, bounds),
            px(size::PICTURE_CHIP_RADIUS).into(),
            image,
            0,
            false,
        );
        chips.push(Chip { bounds, link });
    }
    if let Some((image, link)) = preview {
        paint_preview(&image, &link, &at, offset, t, window);
    }
    chips
}

/// The bigger picture, under the name (over it when there is no room
/// below), within the view.
fn paint_preview(
    image: &Arc<RenderImage>,
    link: &Link,
    at: &Painted,
    offset: i32,
    t: &Theme,
    window: &mut Window,
) {
    let (origin, m, view) = (at.origin, at.m, at.view);
    let (Some(start), Some(end)) = (link.cells.first(), link.cells.last()) else {
        return;
    };
    let h = m.cell.height;
    let natural = image.size(0);
    let (w, ht) = (natural.width.0 as f32, natural.height.0 as f32);
    if w <= 0. || ht <= 0. {
        return;
    }
    let pad = px(size::PICTURE_PAD);
    let scale = (size::PICTURE_PREVIEW_W / w)
        .min(size::PICTURE_PREVIEW_H / ht)
        .min(1.);
    let inner = gpui::size(px(w * scale), px(ht * scale));
    let outer = gpui::size(inner.width + pad * 2., inner.height + pad * 2.);
    let below = origin.y + h * (end.line.0 + offset + 1) as f32 + pad;
    let above = origin.y + h * (start.line.0 + offset) as f32 - pad - outer.height;
    let y = if below + outer.height <= view.bottom() || above < view.top() {
        below
    } else {
        above
    };
    let x = (origin.x + m.cell.width * start.column.0 as f32)
        .min(view.right() - outer.width)
        .max(view.left());
    let card = Bounds::new(point(x, y), outer);
    window.paint_quad(quad(
        card,
        px(size::RADIUS),
        t.raised,
        px(1.),
        t.divider,
        BorderStyle::Solid,
    ));
    let inside = Bounds::new(point(card.left() + pad, card.top() + pad), inner);
    let _ = window.paint_image(
        inside,
        inside,
        px(size::PICTURE_CHIP_RADIUS).into(),
        image.clone(),
        0,
        false,
    );
}

/// Where the picture goes so it covers `b` whole, centred and cropped.
fn cover(image: &RenderImage, b: Bounds<Pixels>) -> Bounds<Pixels> {
    let n = image.size(0);
    let (w, h) = (n.width.0.max(1) as f32, n.height.0.max(1) as f32);
    let scale = (f32::from(b.size.width) / w).max(f32::from(b.size.height) / h);
    let size = gpui::size(px(w * scale), px(h * scale));
    Bounds::new(
        point(
            b.center().x - size.width / 2.,
            b.center().y - size.height / 2.,
        ),
        size,
    )
}

/// A picture, decoded and made no bigger than the preview needs: the
/// first frame of a moving one.
fn small(
    format: gpui::ImageFormat,
    bytes: Vec<u8>,
    svg: gpui::SvgRenderer,
) -> Option<Arc<RenderImage>> {
    let full = gpui::Image::from_bytes(format, bytes)
        .to_image_data(svg)
        .ok()?;
    let n = full.size(0);
    let (w, h) = (n.width.0 as u32, n.height.0 as u32);
    let most = (size::PICTURE_PREVIEW_W.max(size::PICTURE_PREVIEW_H) * 2.) as u32;
    let scale = (most as f32 / w.max(h).max(1) as f32).min(1.);
    let buffer = image::RgbaImage::from_raw(w, h, full.as_bytes(0)?.to_vec())?;
    let (sw, sh) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    let buffer = if (sw, sh) == (w, h) {
        buffer
    } else {
        image::imageops::thumbnail(&buffer, sw, sh)
    };
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

impl Terminals {
    /// The picture at `key` if it has been read; asks for it the first time.
    fn picture(&mut self, key: &Key, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        if self.pictures.want(key) {
            self.read_picture(key.clone(), cx);
        }
        for gone in self.pictures.take_gone() {
            cx.drop_image(gone, None);
        }
        self.pictures.get(key).cloned()
    }

    fn read_picture(&mut self, key: Key, cx: &mut Context<Self>) {
        let Some(format) = pictures::format(&key.1) else {
            return;
        };
        let path = format!("/api/files/file?path={}", http::enc(&key.1));
        let task = self
            .host
            .request(Request::get(NodeId(key.0.clone()), path), cx);
        let svg = cx.svg_renderer();
        cx.spawn(async move |this, cx| {
            let bytes = match task.await {
                Ok(a) if a.ok() => Some(a.body),
                _ => None,
            };
            let image = match bytes {
                Some(b) => {
                    cx.background_executor()
                        .spawn(async move { small(format, b, svg) })
                        .await
                }
                None => None,
            };
            let _ = this.update(cx, |p, cx| {
                p.pictures.arrived(&key, image);
                cx.notify();
            });
        })
        .detach();
    }
}
