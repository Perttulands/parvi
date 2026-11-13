//! Pictures a session's output names, and mermaid diagrams it prints: a
//! chip saying what it is ("image" with a thumbnail, or "diagram") just
//! after the text of the row that names it (a diagram's header), where the
//! row has room; the picture while the pointer rests on the name or the
//! chip; on a click the whole picture in Files beside, as any path opens,
//! or the diagram drawn beside. The grid's text never moves.

use super::super::diagram::{self as panel, DiagramView};
use super::super::page::Terminals;
use crate::diagram;
use crate::http;
use crate::store::pictures::{self, Key};
use crate::term::links::{self, Link, Target};
use crate::term::paint::Metrics;
use crate::term::screen::Screen;
use crate::theme::theme;
use crate::theme::{Theme, size};
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use gpui::{
    App, BorderStyle, Bounds, Context, Entity, Pixels, RenderImage, Window, point, px, quad,
};
use gpui::{AppContext, ImageFormat, Task};
use parvi_app_kit::host::{Answer, Request};
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
    fn key(&self, target: &Target) -> Option<Key> {
        match target {
            Target::Path(p) if pictures::format(p).is_some() => Some(Key::File {
                node: self.node.clone(),
                path: links::resolve(self.cwd.as_deref(), p),
            }),
            Target::Diagram(source) => Some(Key::Diagram(source.clone())),
            _ => None,
        }
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
        .filter_map(|l| Some((place.key(&l.target)?, l)))
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
        let word = match key {
            Key::Diagram(_) => "diagram",
            Key::File { .. } => "image",
        };
        let label = chip_label(word, ch, t, window);
        let thumb = matches!(key, Key::File { .. });
        let pad = px(size::PICTURE_CHIP_PAD);
        let cw = pad * 2.
            + label.width
            + if thumb {
                ch - px(2. * size::PICTURE_CHIP_INSET) + pad
            } else {
                px(0.)
            };
        // A chip follows the row's text a cell after it, and the next one
        // named on the row follows that: beside what names it, however
        // wide the window.
        let grid = s.term.grid();
        let text_end = (0..s.cols)
            .rev()
            .find(|&c| !matches!(grid[GridPoint::new(end.line, Column(c))].c, ' ' | '\0'))
            .map_or(0, |c| c + 1);
        let after_text = origin.x + m.cell.width * (text_end + 1) as f32;
        let left = chips
            .iter()
            .filter(|c: &&Chip| c.bounds.top() == top(row))
            .map(|c| c.bounds.right() + px(size::PICTURE_CHIP_GAP))
            .fold(after_text, Pixels::max);
        // Only over blank cells, within the row: the text stays readable.
        let edge = origin.x + m.cell.width * s.cols as f32;
        if left + cw > edge - px(size::PICTURE_CHIP_INSET) {
            continue;
        }
        let bounds = Bounds::new(point(left, top(row)), gpui::size(cw, ch));
        window.paint_quad(quad(
            bounds,
            px(size::PICTURE_CHIP_RADIUS),
            t.raised,
            px(1.),
            t.divider,
            BorderStyle::Solid,
        ));
        let mut x = left + pad;
        if thumb {
            let inset = px(size::PICTURE_CHIP_INSET);
            let side = ch - inset * 2.;
            let square = Bounds::new(
                point(x - pad + inset * 2., top(row) + inset),
                gpui::size(side, side),
            );
            let _ = window.with_content_mask(Some(gpui::ContentMask { bounds: square }), |w| {
                w.paint_image(
                    square,
                    cover(&image, square),
                    px(size::PICTURE_CHIP_RADIUS).into(),
                    image,
                    0,
                    false,
                )
            });
            x = square.right() + pad;
        }
        let _ = label.paint(
            point(x, top(row)),
            ch,
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );
        chips.push(Chip { bounds, link });
    }
    if let Some((image, link)) = preview {
        paint_preview(&image, &link, &at, offset, t, window);
    }
    chips
}

/// A chip's word, in the app's font, a little smaller than the row.
fn chip_label(word: &'static str, row: Pixels, t: &Theme, window: &mut Window) -> gpui::ShapedLine {
    let run = gpui::TextRun {
        len: word.len(),
        font: gpui::font(parvi_app_kit::theme::FAMILY),
        color: t.text2,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window.text_system().shape_line(
        word.into(),
        px(size::SMALL).min(row * size::PICTURE_CHIP_TEXT),
        &[run],
        None,
    )
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
    Some(shrunk(image::RgbaImage::from_raw(
        w,
        h,
        full.as_bytes(0)?.to_vec(),
    )?))
}

/// Pixels made no bigger than the preview needs, as a picture to draw.
fn shrunk(buffer: image::RgbaImage) -> Arc<RenderImage> {
    let (w, h) = buffer.dimensions();
    let most = (size::PICTURE_PREVIEW_W.max(size::PICTURE_PREVIEW_H) * 2.) as u32;
    let scale = (most as f32 / w.max(h).max(1) as f32).min(1.);
    let (sw, sh) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    let buffer = if (sw, sh) == (w, h) {
        buffer
    } else {
        image::imageops::thumbnail(&buffer, sw, sh)
    };
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

/// How a picture's bytes come: read from its device, or drawn here.
enum Read {
    File(Task<Result<Answer, parvi_core::Error>>),
    Drawn(Task<Option<Arc<RenderImage>>>),
}

impl Terminals {
    fn diagram_colours(&self, cx: &App) -> diagram::Colours {
        let t = theme(cx);
        diagram::Colours {
            bg: t.term_bg_rgb,
            fg: t.term_fg_rgb,
        }
    }

    /// A diagram drawn whole beside the terminals.
    pub(in crate::ui) fn open_diagram(&mut self, source: String, cx: &mut Context<Self>) {
        let colours = self.diagram_colours(cx);
        let density = size::DIAGRAM_START;
        let task = cx.background_executor().spawn(async move {
            let svg = diagram::svg(&source, colours)?;
            let drawn = diagram::paint(&svg, density)?;
            Ok::<_, String>((svg, drawn))
        });
        cx.spawn(async move |this, cx| {
            let made = task.await;
            let _ = this.update(cx, |p, cx| {
                let host = p.host.clone();
                let (svg, drawn) = match made {
                    Ok((svg, d)) => (Arc::from(svg), Ok(panel::Painted::of(d, density))),
                    Err(why) => (Arc::from(""), Err(why)),
                };
                // The panel opens as wide as the diagram at 1:1, room round it.
                let wish = drawn
                    .as_ref()
                    .ok()
                    .map(|d| d.natural.w + 2. * size::DIAGRAM_PAD + 2.);
                let view = cx.new(|cx| DiagramView::new(host.clone(), svg, drawn, cx));
                p.beside_wish = wish.map(|w| (view.entity_id(), w));
                host.show_beside(crate::NAME, view.into(), cx);
            });
        })
        .detach();
    }

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
        let svg = cx.svg_renderer();
        let (format, read) = match &key {
            Key::File { node, path } => {
                let Some(format) = pictures::format(path) else {
                    return;
                };
                let uri = format!("/api/files/file?path={}", http::enc(path));
                let task = self
                    .host
                    .request(Request::get(NodeId(node.clone()), uri), cx);
                (format, Read::File(task))
            }
            Key::Diagram(source) => {
                let (source, colours) = (source.clone(), self.diagram_colours(cx));
                let task = cx.background_executor().spawn(async move {
                    let svg = diagram::svg(&source, colours).ok()?;
                    let d = diagram::paint(&svg, 1.).ok()?;
                    Some(shrunk(d.pixels))
                });
                (ImageFormat::Svg, Read::Drawn(task))
            }
        };
        cx.spawn(async move |this, cx| {
            let image = match read {
                Read::File(task) => match task.await {
                    Ok(a) if a.ok() => {
                        let b = a.body;
                        cx.background_executor()
                            .spawn(async move { small(format, b, svg) })
                            .await
                    }
                    _ => None,
                },
                Read::Drawn(task) => task.await,
            };
            let _ = this.update(cx, |p, cx| {
                p.pictures.arrived(&key, image);
                cx.notify();
            });
        })
        .detach();
    }
}
