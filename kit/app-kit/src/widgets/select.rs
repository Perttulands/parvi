//! Selectable prose: the texts of a column, drawn with the selection laid
//! behind their runs and kept from one frame to the next so a press, a drag
//! or a release can find the byte under the pointer. The column marks
//! itself laid out (`laid_out`) before anything is looked up in it.

use crate::text::text_select::{self, Piece, Selection, Spot};
use gpui::{
    App, ClipboardItem, EntityId, HighlightStyle, Hsla, InteractiveElement, MouseButton, Pixels,
    Point, StyledText, TextLayout, point, px,
};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

/// How far the pointer moves from the press before it selects, so a click
/// that wobbles still follows its link.
const SLOP: f32 = 3.;

#[derive(Default)]
pub struct Selectable {
    pub selection: Selection,
    texts: Vec<(TextLayout, Piece)>,
    /// The column holding this frame's texts is laid out.
    ready: bool,
    /// Where the button went down, until the pointer leaves its slop.
    pressed: Option<Point<Pixels>>,
}

impl Selectable {
    /// A new frame: its texts follow, and nothing can be found until it is
    /// laid out.
    pub fn begin(&mut self) {
        self.texts.clear();
        self.ready = false;
    }

    /// The column holding this frame's texts is laid out.
    pub fn laid_out(&mut self) {
        self.ready = true;
    }

    /// The column's next text, `sep` after the one before it when copied,
    /// with the selected part marked.
    pub fn text(
        &mut self,
        text: String,
        styles: Vec<(Range<usize>, HighlightStyle)>,
        sep: &'static str,
        mark: Hsla,
    ) -> StyledText {
        let i = self.texts.len();
        let styles = match self.selection.within(i, &text) {
            Some(r) if !r.is_empty() => text_select::overlay(&styles, r, |s| HighlightStyle {
                background_color: Some(mark),
                ..s
            }),
            _ => styles,
        };
        let styled = StyledText::new(text.clone()).with_highlights(styles);
        self.texts
            .push((styled.layout().clone(), Piece { text, sep }));
        styled
    }

    /// The left button went down at `p`; whether the selection changed.
    pub fn press(&mut self, p: Point<Pixels>, shift: bool) -> bool {
        let Some(at) = self.hit(p) else {
            let had = !self.selection.is_empty();
            self.selection.clear();
            return had;
        };
        if shift {
            self.selection.extend(at);
            self.pressed = None;
        } else {
            self.selection.press(at);
            self.pressed = Some(p);
        }
        true
    }

    /// The pointer moved to `p` with the button held; whether the
    /// selection changed.
    pub fn drag(&mut self, p: Point<Pixels>) -> bool {
        if !self.selection.dragging {
            return false;
        }
        if let Some(from) = self.pressed {
            let (dx, dy) = (p.x - from.x, p.y - from.y);
            if dx.abs() < px(SLOP) && dy.abs() < px(SLOP) {
                return false;
            }
            self.pressed = None;
        }
        self.hit(p).is_some_and(|at| self.selection.drag(at))
    }

    /// The button is let go: the selected words if a drag selected any.
    pub fn release(&mut self) -> Option<String> {
        self.pressed = None;
        self.selection.release().then(|| self.copied()).flatten()
    }

    /// The selected words, each text after its separator.
    pub fn copied(&self) -> Option<String> {
        let pieces: Vec<Piece> = self.texts.iter().map(|(_, p)| p.clone()).collect();
        self.selection.copied(&pieces)
    }

    /// The spot under `p`, or the nearest: past a text's foot its end,
    /// beside a line that line's nearest end, above the column its start.
    fn hit(&self, p: Point<Pixels>) -> Option<Spot> {
        if !self.ready || self.texts.is_empty() {
            return None;
        }
        let mut above = None;
        for (i, (layout, _)) in self.texts.iter().enumerate() {
            let b = layout.bounds();
            if b.contains(&p) {
                let at = index(layout, &self.texts[i].1.text, p);
                return Some(Spot { text: i, at });
            }
            if b.top() <= p.y {
                above = Some(i);
            }
        }
        let Some(i) = above else {
            return Some(Spot::default());
        };
        let (layout, piece) = &self.texts[i];
        let b = layout.bounds();
        let at = if p.y >= b.bottom() {
            layout.len()
        } else {
            let x = p.x.clamp(b.left(), b.right() - px(1.));
            index(layout, &piece.text, point(x, p.y))
        };
        Some(Spot { text: i, at })
    }
}

/// The mouse on `el`, the area a column of selectable texts scrolls in: a
/// press starts a selection where it lands (Shift extends the one there
/// is), a drag moves its head, and a release hands what a drag selected to
/// `released` (a page copies it when Copy on select is on, as the terminals
/// do). `owner` is the view that draws the column.
pub fn track<E: InteractiveElement>(
    el: E,
    select: &Rc<RefCell<Selectable>>,
    owner: EntityId,
    released: impl Fn(String, &mut App) + 'static,
) -> E {
    let (down, moved, up, out) = (
        select.clone(),
        select.clone(),
        select.clone(),
        select.clone(),
    );
    let released = Rc::new(move |s: &RefCell<Selectable>, cx: &mut App| {
        let Some(text) = s.borrow_mut().release() else {
            return;
        };
        released(text, cx);
    });
    let released_out = released.clone();
    el.on_mouse_down(MouseButton::Left, move |e, _, cx| {
        if down.borrow_mut().press(e.position, e.modifiers.shift) {
            cx.notify(owner);
        }
    })
    .on_mouse_move(move |e, _, cx| {
        if e.pressed_button == Some(MouseButton::Left) && moved.borrow_mut().drag(e.position) {
            cx.notify(owner);
        }
    })
    .on_mouse_up(MouseButton::Left, move |_, _, cx| released(&up, cx))
    .on_mouse_up_out(MouseButton::Left, move |_, _, cx| released_out(&out, cx))
}

/// Puts text on the clipboard: Windows programs expect CRLF between lines
/// (Notepad, Outlook and Windows Terminal copy so); elsewhere LF stays.
pub fn copy(text: String, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(copied(text)));
}

fn copied(text: String) -> String {
    if cfg!(windows) {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text
    }
}

/// The byte nearest `p` in a laid-out text: past a glyph's middle, the
/// boundary after it.
fn index(layout: &TextLayout, text: &str, p: Point<Pixels>) -> usize {
    let at = match layout.index_for_position(p) {
        Ok(at) => at,
        Err(at) => return at,
    };
    let Some(c) = text.get(at..).and_then(|s| s.chars().next()) else {
        return at;
    };
    let next = at + c.len_utf8();
    match (
        layout.position_for_index(at),
        layout.position_for_index(next),
    ) {
        (Some(a), Some(b)) if a.y == b.y && p.x > a.x + (b.x - a.x) / 2. => next,
        _ => at,
    }
}
