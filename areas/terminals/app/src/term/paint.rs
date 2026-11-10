//! Painting one pane: background runs as quads, text in style runs placed at
//! their cell, so the grid holds whatever the fonts' advances are. Wide and
//! non-ASCII glyphs are placed alone at their cell, so a fallback font's
//! advance never shifts the rest of the row.

use super::screen::Screen;
use crate::theme::{FAMILY, Theme, rgb};
use alacritty_terminal::index::Point as GridPoint;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use gpui::{
    App, Bounds, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, Point,
    SharedString, TextRun, UnderlineStyle, Window, fill, point, px, size,
};
use std::sync::Arc;

fn indexed(ansi: &[u32; 16], i: u8) -> u32 {
    match i {
        0..=15 => ansi[i as usize],
        16..=231 => {
            let i = i - 16;
            let v = |x: u8| if x == 0 { 0 } else { 55 + 40 * x as u32 };
            (v(i / 36) << 16) | (v((i / 6) % 6) << 8) | v(i % 6)
        }
        _ => {
            let g = 8 + 10 * (i as u32 - 232);
            (g << 16) | (g << 8) | g
        }
    }
}

fn resolve(c: Color, colors: &Colors, bold: bool, t: &Theme) -> u32 {
    let over =
        |i: usize| colors[i].map(|r| ((r.r as u32) << 16) | ((r.g as u32) << 8) | r.b as u32);
    match c {
        Color::Spec(r) => ((r.r as u32) << 16) | ((r.g as u32) << 8) | r.b as u32,
        Color::Indexed(i) => {
            let i = if bold && i < 8 { i + 8 } else { i };
            over(i as usize).unwrap_or_else(|| indexed(&t.ansi, i))
        }
        Color::Named(n) => {
            let i = n as usize;
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground => {
                    over(i).unwrap_or(t.term_fg_rgb)
                }
                NamedColor::Background => over(i).unwrap_or(t.term_bg_rgb),
                NamedColor::Cursor => t.term_fg_rgb,
                NamedColor::DimForeground => t.ansi[7],
                _ if i < 8 && bold => over(i + 8).unwrap_or(t.ansi[i + 8]),
                _ if i < 16 => over(i).unwrap_or(t.ansi[i]),
                _ => t.ansi[(i - NamedColor::DimBlack as usize).min(7)],
            }
        }
    }
}

thread_local! {
    static FONTS: [Font; 8] = std::array::from_fn(|i| make_font(i & 1 != 0, i & 2 != 0, i & 4 != 0));
}

/// The terminal font. `emoji` is for a wide cell or one with VS16: it takes
/// colour emoji first. A narrow symbol takes the monochrome symbol fonts
/// first, so Claude Code's `⏺` is a one-cell glyph in the text colour, not
/// Segoe UI Emoji's blue tile (Windows Terminal draws it so).
pub fn font(bold: bool, italic: bool, emoji: bool) -> Font {
    FONTS.with(|f| f[bold as usize + 2 * italic as usize + 4 * emoji as usize].clone())
}

fn make_font(bold: bool, italic: bool, emoji: bool) -> Font {
    let (first, second) = if emoji {
        ("Segoe UI Emoji", "Segoe UI Symbol")
    } else {
        ("Segoe UI Symbol", "Segoe UI Emoji")
    };
    Font {
        family: FAMILY.into(),
        // A terminal shows the characters it was sent: `/=` is two cells,
        // not JetBrains Mono's ligature, as in CHROTE's xterm.js.
        features: FontFeatures::disable_ligatures(),
        fallbacks: Some(FontFallbacks(Arc::new(vec![
            "CHROTE Term Symbols".into(),
            first.into(),
            second.into(),
            "Yu Gothic".into(),
            "Malgun Gothic".into(),
            "Noto Color Emoji".into(),
            "Noto Sans CJK JP".into(),
        ]))),
        weight: if bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        },
        style: if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub font_size: Pixels,
    pub cell: gpui::Size<Pixels>,
}

/// The cell of the terminal font at `font_px`, its width snapped to device
/// pixels so a row of cells never drifts.
pub fn metrics(window: &Window, font_px: f32) -> Metrics {
    let ts = window.text_system();
    let id = ts.resolve_font(&font(false, false, false));
    let fs = px(font_px);
    let w = ts.advance(id, fs, 'W').map(|s| s.width).unwrap_or(fs * 0.6);
    let h = (ts.ascent(id, fs) + ts.descent(id, fs).abs()).ceil();
    let sf = window.scale_factor();
    let w = px((f32::from(w) * sf).floor().max(1.) / sf);
    Metrics {
        font_size: fs,
        cell: size(w, h),
    }
}

/// For a pane whose grid someone else sets: the largest font, in half-pixel
/// steps from the Settings size down to the floor, at which the grid fits
/// the box. Below the floor it stays at the floor and the view clips.
pub fn fit(
    window: &Window,
    box_size: gpui::Size<Pixels>,
    cols: usize,
    rows: usize,
    ceiling: f32,
) -> Metrics {
    let fits = |m: Metrics| fits(m, box_size, cols, rows);
    metrics(window, fit_font(ceiling, |f| fits(metrics(window, f))))
}

/// Whether the grid shows whole in the box at these metrics: past the
/// floor it does not, and the view clips.
pub fn fits(m: Metrics, box_size: gpui::Size<Pixels>, cols: usize, rows: usize) -> bool {
    m.cell.width * cols as f32 <= box_size.width && m.cell.height * rows as f32 <= box_size.height
}

/// CHROTE's `fitFontSize`: the largest font no larger than `ceiling` that
/// `fits`, in half pixels, never under the floor (or the ceiling, if that is
/// lower). A grid that fits at one size fits at every smaller one, so it is
/// found by halving: a few measurements, not one per step.
pub fn fit_font(ceiling: f32, fits: impl Fn(f32) -> bool) -> f32 {
    const STEP: f32 = 0.5;
    let floor = crate::theme::size::TERM_FLOOR;
    if ceiling <= floor || fits(ceiling) {
        return ceiling;
    }
    let (mut low, mut high) = (floor, ceiling);
    while high - low > STEP {
        let middle = (low + high).round() / 2.;
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

/// The grid this box holds at this cell: what a seat-holder asks tmux for.
pub fn grid_for(box_size: gpui::Size<Pixels>, m: Metrics) -> (u16, u16) {
    let cols = (f32::from(box_size.width) / f32::from(m.cell.width))
        .floor()
        .max(2.) as u16;
    let rows = (f32::from(box_size.height) / f32::from(m.cell.height))
        .floor()
        .max(1.) as u16;
    (cols, rows)
}

/// Where the grid's top-left goes in a box: at the box's top-left, unless
/// the first `shown` rows (down to the cursor) are taller than the box, when
/// the top is clipped so that row stays at the bottom of the view.
pub fn origin(b: Bounds<Pixels>, m: Metrics, shown: usize) -> Point<Pixels> {
    let over = m.cell.height * shown as f32 - b.size.height;
    point(b.origin.x, b.origin.y - over.max(px(0.)))
}

pub struct Look<'a> {
    pub theme: &'a Theme,
    pub cursor: bool,
    /// A history view and Peek have no cursor and dim nothing.
    pub focused: bool,
    /// The cells of the link under the pointer, underlined.
    pub link: &'a [GridPoint],
}

pub fn paint(
    s: &Screen,
    m: Metrics,
    origin: Point<Pixels>,
    look: Look,
    window: &mut Window,
    cx: &mut App,
) {
    let t = look.theme;
    let content = s.term.renderable_content();
    let colors = content.colors;
    let offset = content.display_offset as i32;
    let selection = content.selection;
    let cursor = content.cursor;
    let cw = m.cell.width;
    let lh = m.cell.height;
    let at = |col: usize, row: i32| point(origin.x + cw * col as f32, origin.y + lh * row as f32);

    struct Batch {
        col: usize,
        text: String,
        runs: Vec<TextRun>,
        style: Option<(bool, bool, bool, Hsla)>,
    }
    let mut batches: Vec<(i32, Batch)> = Vec::new();
    let mut bgs: Vec<(i32, usize, usize, Hsla)> = Vec::new();
    let mut cur: Option<(i32, Batch)> = None;
    for cell in content.display_iter {
        let row = cell.point.line.0 + offset;
        let col = cell.point.column.0;
        let flags = cell.flags;
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let bold = flags.contains(Flags::BOLD);
        let mut fg = resolve(cell.fg, colors, bold, t);
        let mut bgv = resolve(cell.bg, colors, false, t);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bgv);
        }
        let selected = selection
            .is_some_and(|sel| sel.contains(GridPoint::new(cell.point.line, cell.point.column)));
        let wide = flags.contains(Flags::WIDE_CHAR);
        let span = if wide { 2 } else { 1 };
        let mut push_bg = |c: Hsla| match bgs.last_mut() {
            Some((r, c0, n, b)) if *r == row && *b == c && *c0 + *n == col => *n += span,
            _ => bgs.push((row, col, span, c)),
        };
        if bgv != t.term_bg_rgb {
            push_bg(rgb(bgv));
        }
        if selected {
            push_bg(t.selection);
        }
        let ch = if flags.contains(Flags::HIDDEN) {
            ' '
        } else {
            cell.c
        };
        let mut fgc = rgb(fg);
        if flags.contains(Flags::DIM) {
            fgc.a = 0.6;
        }
        let italic = flags.contains(Flags::ITALIC);
        let underline = flags.intersects(Flags::ALL_UNDERLINES);
        let style = (bold, italic, underline, fgc);
        let alone = wide || !ch.is_ascii();
        let fits = matches!(&cur, Some((r, b)) if *r == row && !alone && b.text.is_ascii() && b.col + b.text.len() == col);
        if !fits {
            if let Some(done) = cur.take() {
                batches.push(done);
            }
            cur = Some((
                row,
                Batch {
                    col,
                    text: String::new(),
                    runs: Vec::new(),
                    style: None,
                },
            ));
        }
        let Some((_, b)) = cur.as_mut() else { continue };
        let mut zero = String::new();
        if let Some(z) = cell.zerowidth() {
            zero.extend(z.iter());
        }
        let len = ch.len_utf8() + zero.len();
        let emoji = wide || zero.contains('\u{fe0f}');
        b.text.push(ch);
        b.text.push_str(&zero);
        match b.runs.last_mut() {
            Some(last) if b.style == Some(style) => last.len += len,
            _ => {
                b.runs.push(TextRun {
                    len,
                    font: font(bold, italic, emoji),
                    color: fgc,
                    background_color: None,
                    underline: underline.then_some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(fgc),
                        wavy: false,
                    }),
                    strikethrough: None,
                });
                b.style = Some(style);
            }
        }
        if alone && let Some(done) = cur.take() {
            batches.push(done);
        }
    }
    if let Some(done) = cur.take() {
        batches.push(done);
    }

    for (row, col, n, c) in bgs {
        window.paint_quad(fill(Bounds::new(at(col, row), size(cw * n as f32, lh)), c));
    }
    if look.cursor && offset == 0 && cursor.shape != CursorShape::Hidden {
        let p = at(cursor.point.column.0, cursor.point.line.0);
        let c = t.cursor;
        if look.focused {
            let b = match cursor.shape {
                CursorShape::Beam => Bounds::new(p, size(px(2.), lh)),
                CursorShape::Underline => {
                    Bounds::new(point(p.x, p.y + lh - px(2.)), size(cw, px(2.)))
                }
                _ => Bounds::new(p, size(cw, lh)),
            };
            window.paint_quad(fill(b, Hsla { a: 0.7, ..c }));
        } else {
            // An unfocused terminal shows where its cursor is, hollow.
            window.paint_quad(gpui::outline(
                Bounds::new(p, size(cw, lh)),
                Hsla { a: 0.5, ..c },
                gpui::BorderStyle::Solid,
            ));
        }
    }
    let rule = rgb(t.term_fg_rgb);
    for p in look.link {
        let under = at(p.column.0, p.line.0 + offset);
        window.paint_quad(fill(
            Bounds::new(point(under.x, under.y + lh - px(1.)), size(cw, px(1.))),
            rule,
        ));
    }
    let ts = window.text_system().clone();
    for (row, b) in batches {
        if b.text.trim().is_empty() {
            continue;
        }
        let line = ts.shape_line(SharedString::from(b.text), m.font_size, &b.runs, Some(cw));
        let _ = line.paint(at(b.col, row), lh, gpui::TextAlign::Left, None, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tall_grid_keeps_the_cursor_row_in_view() {
        let b = Bounds::new(point(px(0.), px(100.)), size(px(80.), px(40.)));
        let m = Metrics {
            font_size: px(10.),
            cell: size(px(8.), px(10.)),
        };
        assert_eq!(
            origin(b, m, 3).y,
            px(100.),
            "a prompt near the top: no clipping"
        );
        assert_eq!(origin(b, m, 6).y, px(80.), "row 6 ends at the box's bottom");
    }

    #[test]
    fn the_font_fits_in_half_pixels_and_never_under_eleven() {
        // A grid that fits at 13.5 px and not at 14.
        assert_eq!(fit_font(20., |f| f <= 13.5), 13.5);
        assert_eq!(fit_font(16., |f| f <= 16.), 16., "the ceiling when it fits");
        assert_eq!(fit_font(20., |_| false), 11., "the floor, clipped");
        assert_eq!(fit_font(20., |f| f <= 11.2), 11.);
        assert_eq!(fit_font(10., |_| false), 10., "a ceiling under the floor");
    }
}
