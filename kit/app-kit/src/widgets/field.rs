//! The one text field: single line or a wrapping note, with IME, the
//! clipboard and word moves. It owns its text and reports what happened;
//! what Enter or Up means is its owner's business.

use crate::theme::{FAMILY, alpha, size, theme};
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions,
    div, fill, point, prelude::*, px, relative, size as sz,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

actions!(
    field,
    [
        Backspace,
        BackspaceWord,
        Delete,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        Paste,
        Cut,
        Copy,
        Enter,
        ShiftEnter,
        CtrlEnter,
        Up,
        Down,
        Tab,
    ]
);

pub const CONTEXT: &str = "Field";

pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("ctrl-backspace", BackspaceWord, c),
        KeyBinding::new("ctrl-w", BackspaceWord, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("ctrl-left", WordLeft, c),
        KeyBinding::new("ctrl-right", WordRight, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
        KeyBinding::new("ctrl-a", SelectAll, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("ctrl-v", Paste, c),
        KeyBinding::new("shift-insert", Paste, c),
        KeyBinding::new("ctrl-x", Cut, c),
        KeyBinding::new("ctrl-c", Copy, c),
        KeyBinding::new("enter", Enter, c),
        KeyBinding::new("shift-enter", ShiftEnter, c),
        KeyBinding::new("ctrl-enter", CtrlEnter, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("tab", Tab, c),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldEvent {
    Changed,
    /// Enter.
    Submit,
    /// Ctrl+Enter.
    SubmitAlt,
    Up,
    Down,
    Tab,
    Focused,
}

/// An attachment pasted into a field configured to accept one.
pub struct AttachmentEvent(pub ClipboardItem);

pub struct TextField {
    focus: FocusHandle,
    pub text: String,
    placeholder: SharedString,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    multiline: bool,
    attachments: bool,
    /// Enter starts a new line, as in a text area, instead of submitting.
    prose: bool,
    /// A mono, smaller field for paths and flags looks like the rest.
    text_size: f32,
    lines: Vec<(usize, WrappedLine)>,
    bounds: Option<Bounds<Pixels>>,
    /// How far a single line is scrolled left so the caret stays in view.
    scroll: Pixels,
    selecting: bool,
    was_focused: bool,
}

impl EventEmitter<FieldEvent> for TextField {}
impl EventEmitter<AttachmentEvent> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextField {
    pub fn new(cx: &mut Context<Self>, placeholder: &str) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            text: String::new(),
            placeholder: placeholder.to_string().into(),
            selected: 0..0,
            reversed: false,
            marked: None,
            multiline: false,
            attachments: false,
            prose: false,
            text_size: size::TEXT,
            lines: vec![],
            bounds: None,
            scroll: px(0.),
            selecting: false,
            was_focused: false,
        }
    }

    pub fn attachments(mut self) -> Self {
        self.attachments = true;
        self
    }

    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    /// A text area: multiline, and Enter starts a new line.
    pub fn prose(mut self) -> Self {
        self.multiline = true;
        self.prose = true;
        self
    }

    /// Where the field was last drawn, in window coordinates.
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.bounds
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    pub fn set_placeholder(&mut self, placeholder: &str) {
        self.placeholder = placeholder.to_string().into();
    }

    /// Replaces the text, cursor at the end, all of it selected if asked.
    pub fn set(&mut self, text: &str, select: bool, cx: &mut Context<Self>) {
        self.text = text.to_string();
        self.selected = if select {
            0..self.text.len()
        } else {
            self.text.len()..self.text.len()
        };
        self.reversed = false;
        self.marked = None;
        cx.notify();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    fn move_to(&mut self, at: usize, cx: &mut Context<Self>) {
        self.selected = at..at;
        self.reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, at: usize, cx: &mut Context<Self>) {
        if self.reversed {
            self.selected.start = at;
        } else {
            self.selected.end = at;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    fn previous(&self, at: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .rev()
            .find_map(|(i, _)| (i < at).then_some(i))
            .unwrap_or(0)
    }

    fn next(&self, at: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .find_map(|(i, _)| (i > at).then_some(i))
            .unwrap_or(self.text.len())
    }

    fn word_left(&self, at: usize) -> usize {
        let mut last = 0;
        for (i, w) in self.text.split_word_bound_indices() {
            if i >= at {
                break;
            }
            if !w.trim().is_empty() {
                last = i;
            }
        }
        last
    }

    fn word_right(&self, at: usize) -> usize {
        for (i, w) in self.text.split_word_bound_indices() {
            let end = i + w.len();
            if end > at && !w.trim().is_empty() {
                return end;
            }
        }
        self.text.len()
    }

    fn line_start(&self, at: usize) -> usize {
        if !self.multiline {
            return 0;
        }
        self.text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self, at: usize) -> usize {
        if !self.multiline {
            return self.text.len();
        }
        self.text[at..]
            .find('\n')
            .map(|i| at + i)
            .unwrap_or(self.text.len())
    }

    fn insert(&mut self, s: &str, cx: &mut Context<Self>) {
        let r = self.marked.take().unwrap_or(self.selected.clone());
        self.text.replace_range(r.clone(), s);
        let at = r.start + s.len();
        self.selected = at..at;
        self.reversed = false;
        self.changed(cx);
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let p = self.previous(self.cursor());
            self.select_to(p, cx);
        }
        self.insert("", cx);
    }
    fn backspace_word(&mut self, _: &BackspaceWord, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let p = self.word_left(self.cursor());
            self.select_to(p, cx);
        }
        self.insert("", cx);
    }
    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let n = self.next(self.cursor());
            self.select_to(n, cx);
        }
        self.insert("", cx);
    }
    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let at = if self.selected.is_empty() {
            self.previous(self.cursor())
        } else {
            self.selected.start
        };
        self.move_to(at, cx);
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let at = if self.selected.is_empty() {
            self.next(self.cursor())
        } else {
            self.selected.end
        };
        self.move_to(at, cx);
    }
    fn word_left_a(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.word_left(self.cursor());
        self.move_to(at, cx);
    }
    fn word_right_a(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.word_right(self.cursor());
        self.move_to(at, cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.previous(self.cursor());
        self.select_to(at, cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.next(self.cursor());
        self.select_to(at, cx);
    }
    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.word_left(self.cursor());
        self.select_to(at, cx);
    }
    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.word_right(self.cursor());
        self.select_to(at, cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0..self.text.len();
        self.reversed = false;
        cx.notify();
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.line_start(self.cursor());
        self.move_to(at, cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.line_end(self.cursor());
        self.move_to(at, cx);
    }
    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.line_start(self.cursor());
        self.select_to(at, cx);
    }
    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.line_end(self.cursor());
        self.select_to(at, cx);
    }
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        if self.attachments
            && item.entries().iter().any(|e| {
                matches!(
                    e,
                    gpui::ClipboardEntry::Image(_) | gpui::ClipboardEntry::ExternalPaths(_)
                )
            })
        {
            cx.emit(AttachmentEvent(item));
        } else if let Some(t) = item.text() {
            let t = if self.multiline {
                t.replace("\r\n", "\n")
            } else {
                t.replace(['\r', '\n'], " ")
            };
            self.insert(&t, cx);
        }
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text[self.selected.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text[self.selected.clone()].to_string(),
            ));
            self.insert("", cx);
        }
    }
    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.prose {
            return self.insert("\n", cx);
        }
        cx.emit(FieldEvent::Submit);
    }
    fn shift_enter(&mut self, _: &ShiftEnter, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.insert("\n", cx);
        } else {
            cx.emit(FieldEvent::Submit);
        }
    }
    fn ctrl_enter(&mut self, _: &CtrlEnter, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FieldEvent::SubmitAlt);
    }
    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline && self.text[..self.cursor()].contains('\n') {
            let start = self.line_start(self.cursor());
            let col = self.cursor() - start;
            let prev = self.line_start(start - 1);
            let at = (prev + col).min(start - 1);
            self.move_to(self.text.floor_char_boundary(at), cx);
            return;
        }
        cx.emit(FieldEvent::Up);
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline && self.text[self.cursor()..].contains('\n') {
            let start = self.line_start(self.cursor());
            let col = self.cursor() - start;
            let next = self.line_end(self.cursor()) + 1;
            let at = (next + col).min(self.line_end(next));
            self.move_to(self.text.floor_char_boundary(at), cx);
            return;
        }
        cx.emit(FieldEvent::Down);
    }
    fn tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FieldEvent::Tab);
    }

    /// Where the text starts drawing: the box, less the sideways scroll.
    fn origin(&self) -> Option<Point<Pixels>> {
        self.bounds
            .map(|b| point(b.origin.x - self.scroll, b.origin.y))
    }

    fn index_at(&self, p: Point<Pixels>, line_height: Pixels) -> usize {
        let Some(o) = self.origin() else { return 0 };
        let mut y = o.y;
        for (start, line) in &self.lines {
            let h = line.size(line_height).height.max(line_height);
            if p.y < y + h || std::ptr::eq(line, &self.lines.last().unwrap().1) {
                let local = point(p.x - o.x, (p.y - y).max(px(0.)));
                let i = line
                    .closest_index_for_position(local, line_height)
                    .unwrap_or_else(|i| i);
                return (start + i).min(self.text.len());
            }
            y += h;
        }
        self.text.len()
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.selecting = true;
        let at = self.index_at(e.position, line_height(self.text_size));
        if e.click_count >= 2 {
            self.selected = self.word_left(self.next(at).min(self.text.len()))..self.word_right(at);
            self.reversed = false;
            cx.notify();
        } else if e.modifiers.shift {
            self.select_to(at, cx);
        } else {
            self.move_to(at, cx);
        }
    }
    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }
    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && e.pressed_button == Some(MouseButton::Left) {
            let at = self.index_at(e.position, line_height(self.text_size));
            self.select_to(at, cx);
        }
    }

    fn utf16_to(&self, o: usize) -> usize {
        let mut u8i = 0;
        let mut u16i = 0;
        for c in self.text.chars() {
            if u16i >= o {
                break;
            }
            u16i += c.len_utf16();
            u8i += c.len_utf8();
        }
        u8i
    }
    fn to_utf16(&self, o: usize) -> usize {
        self.text[..o.min(self.text.len())].encode_utf16().count()
    }
    fn range_from16(&self, r: &Range<usize>) -> Range<usize> {
        self.utf16_to(r.start)..self.utf16_to(r.end)
    }
    fn range_to16(&self, r: &Range<usize>) -> Range<usize> {
        self.to_utf16(r.start)..self.to_utf16(r.end)
    }

    fn position_of(&self, at: usize, line_height: Pixels) -> Option<Point<Pixels>> {
        let o = self.origin()?;
        let mut y = o.y;
        for (start, line) in &self.lines {
            let end = start + line.len();
            if at >= *start && at <= end {
                let p = line.position_for_index(at - start, line_height)?;
                return Some(point(o.x + p.x, y + p.y));
            }
            y += line.size(line_height).height.max(line_height);
        }
        None
    }
}

fn line_height(text: f32) -> Pixels {
    px((text * 1.5).round())
}

const CARET: Pixels = px(1.5);

/// The sideways scroll that keeps a single line's caret in its box: it moves
/// only as far as the caret left the box, and never shows empty space past
/// the text's end while there is text before the box.
fn keep_in_view(scroll: Pixels, caret: Pixels, text: Pixels, width: Pixels) -> Pixels {
    let room = (width - CARET).max(px(0.));
    let mut s = scroll;
    if caret - s > room {
        s = caret - room;
    }
    if caret < s {
        s = caret;
    }
    s.min((text + CARET - width).max(px(0.))).max(px(0.))
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        r: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let r = self.range_from16(&r);
        actual.replace(self.range_to16(&r));
        Some(self.text[r].to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to16(&self.selected),
            reversed: self.reversed,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to16(r))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.multiline && (text == "\n" || text == "\r") {
            return;
        }
        if let Some(r) = r {
            self.marked = None;
            self.selected = self.range_from16(&r);
        }
        self.insert(text, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        sel: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let r = r
            .map(|r| self.range_from16(&r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        self.text.replace_range(r.clone(), text);
        self.marked = (!text.is_empty()).then(|| r.start..r.start + text.len());
        let start16 = self.to_utf16(r.start);
        let len16 = text.encode_utf16().count();
        self.selected = sel
            .map(|s| self.range_from16(&(start16 + s.start.min(len16)..start16 + s.end.min(len16))))
            .unwrap_or(r.start + text.len()..r.start + text.len());
        self.reversed = false;
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        r: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let r = self.range_from16(&r);
        let lh = line_height(self.text_size);
        let p = self.position_of(r.start, lh)?;
        Some(Bounds::new(p, sz(px(2.), lh)))
    }
    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let i = self.index_at(p, line_height(self.text_size));
        Some(self.to_utf16(i))
    }
}

struct TextElement {
    field: Entity<TextField>,
}

struct Prepaint {
    lines: Vec<(usize, WrappedLine)>,
    quads: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl TextElement {
    fn shape(
        &self,
        width: Option<Pixels>,
        window: &mut Window,
        cx: &App,
    ) -> Vec<(usize, WrappedLine)> {
        let f = self.field.read(cx);
        let t = theme(cx);
        let (text, color) = if f.text.is_empty() && f.marked.is_none() {
            (f.placeholder.to_string(), t.dim)
        } else {
            (f.text.clone(), t.text)
        };
        let font = gpui::font(FAMILY);
        let mut runs = vec![];
        let base = TextRun {
            len: 0,
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        match &f.marked {
            Some(m) if !f.text.is_empty() => {
                runs.push(TextRun {
                    len: m.start,
                    ..base.clone()
                });
                runs.push(TextRun {
                    len: m.end - m.start,
                    underline: Some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..base.clone()
                });
                runs.push(TextRun {
                    len: text.len() - m.end,
                    ..base
                });
            }
            _ => runs.push(TextRun {
                len: text.len(),
                ..base
            }),
        }
        runs.retain(|r| r.len > 0);
        let shown = if text.is_empty() {
            " ".to_string()
        } else {
            text.clone()
        };
        if runs.is_empty() {
            runs.push(TextRun {
                len: 1,
                font: gpui::font(FAMILY),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }
        let wrap = if f.multiline { width } else { None };
        let shaped = window
            .text_system()
            .shape_text(shown.clone().into(), px(f.text_size), &runs, wrap, None)
            .unwrap_or_default();
        let mut start = 0;
        let mut out = vec![];
        for line in shaped {
            let len = line.len();
            out.push((start, line));
            start += len + 1;
        }
        // A placeholder has no offsets in the text.
        if f.text.is_empty() {
            for (s, _) in &mut out {
                *s = 0;
            }
        }
        out
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let f = self.field.read(cx);
        let lh = line_height(f.text_size);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if f.multiline {
            // Height follows the text: measured against the last known width.
            let width = f.bounds.map(|b| b.size.width);
            let lines = self.shape(width, window, cx);
            let h: Pixels = lines
                .iter()
                .map(|(_, l)| l.size(lh).height.max(lh))
                .fold(px(0.), |a, b| a + b);
            style.size.height = h.max(lh * 3.).into();
        } else {
            style.size.height = lh.into();
        }
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let lines = self.shape(Some(bounds.size.width), window, cx);
        let scroll = self.field.update(cx, |f, _| {
            f.scroll = match lines.first() {
                Some((_, line)) if !f.multiline && !f.text.is_empty() => {
                    let lh = line_height(f.text_size);
                    let caret = line
                        .position_for_index(f.cursor(), lh)
                        .map_or(line.width(), |p| p.x);
                    keep_in_view(f.scroll, caret, line.width(), bounds.size.width)
                }
                _ => px(0.),
            };
            f.scroll
        });
        let origin = point(bounds.origin.x - scroll, bounds.origin.y);
        let f = self.field.read(cx);
        let t = theme(cx);
        let lh = line_height(f.text_size);
        let mut quads = vec![];
        let mut cursor = None;
        let pos = |at: usize| -> Option<Point<Pixels>> {
            let mut y = origin.y;
            for (start, line) in &lines {
                if at >= *start && at <= start + line.len() {
                    let p = line.position_for_index(at - start, lh)?;
                    return Some(point(origin.x + p.x, y + p.y));
                }
                y += line.size(lh).height.max(lh);
            }
            None
        };
        if !f.text.is_empty() || f.marked.is_some() {
            if f.selected.is_empty() {
                if let Some(p) = pos(f.cursor()) {
                    cursor = Some(fill(
                        Bounds::new(point(p.x, p.y + px(2.)), sz(CARET, lh - px(4.))),
                        t.accent,
                    ));
                }
            } else {
                // One band per visual row the selection crosses.
                let (a, b) = (f.selected.start, f.selected.end);
                let mut at = a;
                while at < b {
                    let Some(p0) = pos(at) else { break };
                    let mut end = at;
                    let mut p1 = p0;
                    while end < b {
                        let n = f.text[end..]
                            .chars()
                            .next()
                            .map(|c| end + c.len_utf8())
                            .unwrap_or(b);
                        match pos(n) {
                            Some(q) if q.y == p0.y => {
                                end = n;
                                p1 = q;
                            }
                            _ => {
                                end = n;
                                break;
                            }
                        }
                    }
                    let w = (p1.x - p0.x).max(px(4.));
                    quads.push(fill(
                        Bounds::new(point(p0.x, p0.y + px(2.)), sz(w, lh - px(4.))),
                        alpha(t.accent, 0.3),
                    ));
                    at = end.max(at + 1);
                }
            }
        } else if let Some(p) = pos(0) {
            cursor = Some(fill(
                Bounds::new(point(p.x, p.y + px(2.)), sz(CARET, lh - px(4.))),
                t.accent,
            ));
        }
        Prepaint {
            lines,
            quads,
            cursor,
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        pre: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.field.read(cx).focus.clone();
        let lh = line_height(self.field.read(cx).text_size);
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.field.clone()),
            cx,
        );
        let x = bounds.origin.x - self.field.read(cx).scroll;
        // The text stays in its box, however far it runs.
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for q in pre.quads.drain(..) {
                window.paint_quad(q);
            }
            let mut y = bounds.origin.y;
            for (_, line) in &pre.lines {
                let _ = line.paint(point(x, y), lh, gpui::TextAlign::Left, None, window, cx);
                y += line.size(lh).height.max(lh);
            }
            if focus.is_focused(window)
                && let Some(c) = pre.cursor.take()
            {
                window.paint_quad(c);
            }
        });
        let lines = std::mem::take(&mut pre.lines);
        let resized = self.field.update(cx, |f, _| {
            f.lines = lines;
            let resized = f.bounds.map(|b| b.size.width) != Some(bounds.size.width);
            f.bounds = Some(bounds);
            f.multiline && resized
        });
        // A multiline field's height is measured at its last width: a new
        // width lays it out again on the next frame (a notify while drawing
        // is lost), or it keeps its first, unwrapped height.
        if resized {
            let field = self.field.clone();
            window.on_next_frame(move |_, cx| field.update(cx, |_, cx| cx.notify()));
        }
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        if focused && !self.was_focused {
            cx.emit(FieldEvent::Focused);
        }
        self.was_focused = focused;
        let t = theme(cx);
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::backspace_word))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left_a))
            .on_action(cx.listener(Self::word_right_a))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::shift_enter))
            .on_action(cx.listener(Self::ctrl_enter))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::tab))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .w_full()
            .px(px(8.))
            .py(px(if self.multiline { 6. } else { 3. }))
            .bg(t.bg)
            .border_1()
            .border_color(if focused { t.accent } else { t.divider })
            .rounded(px(size::RADIUS))
            .text_size(px(self.text_size))
            .child(TextElement { field: cx.entity() })
    }
}

/// Moves the keyboard into a field.
pub fn focus(field: &Entity<TextField>, window: &mut Window, cx: &mut App) {
    let handle = field.read(cx).focus.clone();
    window.focus(&handle, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn typing_moving_and_deleting(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(crate::theme::Theme::violet()));
        let f = cx.new(|cx| TextField::new(cx, ""));
        f.update(cx, |f, cx| {
            f.set("hello brave world", false, cx);
            assert_eq!(f.word_left(f.text.len()), 12);
            assert_eq!(f.word_left(12), 6);
            assert_eq!(f.word_right(0), 5);
            f.selected = 6..6;
            f.insert("new ", cx);
            assert_eq!(f.text, "hello new brave world");
            assert_eq!(f.to_utf16(f.text.len()), 21);
        });
    }

    #[gpui::test]
    fn send_field_pastes_text_and_emits_clipboard_attachments(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(|cx| cx.set_global(crate::theme::Theme::violet()));
        let (f, cx) = cx.add_window_view(|_, cx| TextField::new(cx, "").multiline().attachments());
        let received = Rc::new(RefCell::new(Vec::new()));
        let seen = received.clone();
        cx.update(|_, cx| {
            cx.subscribe(&f, move |_, event: &AttachmentEvent, _| {
                seen.borrow_mut().push(event.0.clone());
            })
            .detach()
        });
        cx.run_until_parked();
        cx.write_to_clipboard(ClipboardItem::new_string("hello\r\nworld".into()));
        cx.update(|window, cx| f.update(cx, |f, cx| f.paste(&Paste, window, cx)));
        assert_eq!(f.read_with(cx, |f, _| f.text.clone()), "hello\nworld");
        let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, vec![1, 2, 3]);
        cx.write_to_clipboard(ClipboardItem::new_image(&image));
        cx.update(|window, cx| f.update(cx, |f, cx| f.paste(&Paste, window, cx)));
        assert_eq!(
            received.borrow().as_slice(),
            &[ClipboardItem::new_image(&image)]
        );
        assert_eq!(f.read_with(cx, |f, _| f.text.clone()), "hello\nworld");
        let pasted = ClipboardItem {
            entries: vec![gpui::ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                [std::path::PathBuf::from("/tmp/example.txt")]
                    .into_iter()
                    .collect(),
            ))],
        };
        cx.write_to_clipboard(pasted.clone());
        cx.update(|window, cx| f.update(cx, |f, cx| f.paste(&Paste, window, cx)));
        assert_eq!(received.borrow().last(), Some(&pasted));
        assert_eq!(f.read_with(cx, |f, _| f.text.clone()), "hello\nworld");
    }

    #[test]
    fn a_long_line_scrolls_to_keep_its_caret_in_the_box() {
        let w = px(100.);
        // Text that fits never scrolls.
        assert_eq!(keep_in_view(px(0.), px(60.), px(60.), w), px(0.));
        // Typing past the right edge scrolls just enough to show the caret.
        let s = keep_in_view(px(0.), px(250.), px(250.), w);
        assert_eq!(s, px(250.) + CARET - w);
        // Moving back inside the box leaves the scroll where it was.
        assert_eq!(keep_in_view(s, px(200.), px(250.), w), s);
        // Moving past the left edge brings the caret to it.
        assert_eq!(keep_in_view(s, px(40.), px(250.), w), px(40.));
        // Home shows the start again.
        assert_eq!(keep_in_view(s, px(0.), px(250.), w), px(0.));
        // Deleting the tail pulls the text back rather than show empty space.
        assert_eq!(keep_in_view(s, px(80.), px(80.), w), px(0.));
    }

    #[gpui::test]
    fn a_drawn_field_scrolls_with_its_caret(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(crate::theme::Theme::violet()));
        let (f, cx) = cx.add_window_view(|_, cx| TextField::new(cx, ""));
        let long = "/srv/.claude/worktrees/agent/projects/beta".repeat(20);
        f.update(cx, |f, cx| f.set(&long, false, cx));
        cx.run_until_parked();
        let (scroll, width) = f.read_with(cx, |f, _| (f.scroll, f.bounds.unwrap().size.width));
        assert!(scroll > px(0.), "the end of a long path is in view");
        let caret = f.read_with(cx, |f, _| {
            f.position_of(f.text.len(), line_height(f.text_size))
        });
        let b = f.read_with(cx, |f, _| f.bounds.unwrap());
        let caret = caret.unwrap();
        assert!(caret.x >= b.origin.x && caret.x + CARET <= b.origin.x + width);
        f.update(cx, |f, cx| f.move_to(0, cx));
        cx.run_until_parked();
        assert_eq!(f.read_with(cx, |f, _| f.scroll), px(0.));
    }
}
