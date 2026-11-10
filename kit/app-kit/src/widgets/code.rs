//! A file's text with line numbers: read, selected and copied, and edited
//! when asked. CHROTE's editor rules: Tab inserts two spaces, Ctrl+S saves,
//! Escape asks (the owner decides what it means). Only the lines in view
//! are shaped, so a long file costs what a screenful does. Code is
//! coloured by its language ([`super::syntax`]). Any page may make one
//! (`cx.new(Code::new)`) and hear its [`CodeEvent`]s.

use super::syntax::{self, Carry, Lang, Token};
use crate::theme::{FAMILY, Theme, alpha, rgb, size, theme};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, Hsla, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, ScrollEvent, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, Window, div, fill, point, prelude::*, px,
    relative, size as sz,
};
use std::ops::Range;
use std::time::{Duration, Instant};

/// A tab shows as this many spaces.
const TAB_SHOWN: usize = 4;
/// Tab inserts this (CHROTE Editor.tsx).
const INDENT: &str = "  ";
/// Typing within this long of the last edit undoes together.
const UNDO_GROUP: Duration = Duration::from_millis(700);
const UNDO_DEPTH: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeEvent {
    Changed,
    Save,
    /// Escape: the owner asks or closes.
    Escape,
}

pub struct Code {
    focus: FocusHandle,
    text: String,
    /// Where each line starts.
    starts: Vec<usize>,
    /// The language it is coloured as, and what each line starts inside;
    /// none past `syntax::MAX_BYTES`.
    lang: Option<&'static Lang>,
    carries: Vec<Carry>,
    pub editable: bool,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    /// The first line in view, fractional, and the horizontal offset.
    top: f32,
    left: Pixels,
    undo: Vec<(String, Range<usize>)>,
    redo: Vec<(String, Range<usize>)>,
    last_edit: Option<Instant>,
    selecting: bool,
    /// Where the text (after the gutter) was last drawn, and its metrics.
    area: Option<Bounds<Pixels>>,
    char_w: Pixels,
}

impl EventEmitter<CodeEvent> for Code {}

impl Focusable for Code {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn line_height() -> Pixels {
    px((size::TEXT * 1.55).round())
}

/// Line starts of `text`.
fn starts_of(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// A line as shown (tabs as spaces), and how a byte in it maps to a shown
/// byte.
fn expand(line: &str) -> String {
    line.replace('\t', &" ".repeat(TAB_SHOWN))
}

fn to_shown(line: &str, at: usize) -> usize {
    at + line[..at.min(line.len())].matches('\t').count() * (TAB_SHOWN - 1)
}

fn from_shown(line: &str, shown: usize) -> usize {
    let mut s = 0;
    for (i, c) in line.char_indices() {
        let w = if c == '\t' { TAB_SHOWN } else { c.len_utf8() };
        if s + w > shown {
            return if shown - s > w / 2 && c == '\t' {
                i + 1
            } else {
                i
            };
        }
        s += w;
    }
    line.len()
}

impl Code {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            text: String::new(),
            starts: vec![0],
            lang: None,
            carries: vec![],
            editable: false,
            selected: 0..0,
            reversed: false,
            marked: None,
            top: 0.,
            left: px(0.),
            undo: vec![],
            redo: vec![],
            last_edit: None,
            selecting: false,
            area: None,
            char_w: px(8.),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Colours the text as `lang`, or not at all.
    pub fn set_lang(&mut self, lang: Option<&'static Lang>, cx: &mut Context<Self>) {
        self.lang = lang;
        self.reindex();
        cx.notify();
    }

    /// Finds the lines again after the text changed.
    fn reindex(&mut self) {
        self.starts = starts_of(&self.text);
        self.carries = match self.lang {
            Some(lang) if self.text.len() <= syntax::MAX_BYTES => {
                syntax::carries(lang, self.text.split('\n'))
            }
            _ => vec![],
        };
    }

    /// Line `n`'s coloured stretches, in bytes of the line.
    fn tokens(&self, n: usize) -> Vec<(Range<usize>, Token)> {
        match (self.lang, self.carries.get(n)) {
            (Some(lang), Some(carry)) => {
                syntax::spans(lang, &self.text[self.line_range(n)], *carry).0
            }
            _ => vec![],
        }
    }

    /// Shows new text from its first line, nothing selected.
    pub fn set(&mut self, text: &str, cx: &mut Context<Self>) {
        self.text = text.to_string();
        self.reindex();
        self.selected = 0..0;
        self.reversed = false;
        self.marked = None;
        self.top = 0.;
        self.left = px(0.);
        self.undo.clear();
        self.redo.clear();
        cx.notify();
    }

    /// Gives the view the keyboard.
    pub fn focus(this: &Entity<Code>, window: &mut Window, cx: &mut App) {
        let handle = this.read(cx).focus.clone();
        window.focus(&handle, cx);
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// The first line in view (0-based).
    pub fn top_line(&self) -> usize {
        self.top.max(0.) as usize
    }

    /// Turns editing on or off. On, the caret lands at the start of the
    /// line being read, and the keyboard comes here.
    pub fn edit(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.editable = on;
        if on {
            let at = self.starts[self.top_line().min(self.starts.len() - 1)];
            self.selected = at..at;
            self.reversed = false;
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Scrolls so line `n` (0-based) is the first in view.
    pub fn show_line(&mut self, n: usize, cx: &mut Context<Self>) {
        self.top = n.min(self.starts.len().saturating_sub(1)) as f32;
        cx.notify();
    }

    fn line_of(&self, at: usize) -> usize {
        match self.starts.binary_search(&at) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    fn line_range(&self, n: usize) -> Range<usize> {
        let start = self.starts[n];
        let end = self
            .starts
            .get(n + 1)
            .map(|s| s - 1)
            .unwrap_or(self.text.len());
        start..end
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    fn rows_in_view(&self) -> usize {
        self.area
            .map(|b| (b.size.height / line_height()).floor() as usize)
            .unwrap_or(20)
            .max(1)
    }

    fn reveal(&mut self) {
        let line = self.line_of(self.cursor()) as f32;
        let rows = self.rows_in_view() as f32;
        if line < self.top {
            self.top = line;
        } else if line >= self.top + rows {
            self.top = line - rows + 1.;
        }
        if let Some(b) = self.area {
            let n = self.line_of(self.cursor());
            let r = self.line_range(n);
            let col = to_shown(&self.text[r.clone()], self.cursor() - r.start);
            let x = self.char_w * col as f32;
            if x < self.left {
                self.left = (x - self.char_w * 8.).max(px(0.));
            } else if x > self.left + b.size.width - self.char_w * 2. {
                self.left = x - b.size.width + self.char_w * 8.;
            }
        }
    }

    fn move_to(&mut self, at: usize, select: bool, cx: &mut Context<Self>) {
        let at = self.text.floor_char_boundary(at.min(self.text.len()));
        if select {
            if self.reversed {
                self.selected.start = at;
            } else {
                self.selected.end = at;
            }
            if self.selected.end < self.selected.start {
                self.reversed = !self.reversed;
                self.selected = self.selected.end..self.selected.start;
            }
        } else {
            self.selected = at..at;
            self.reversed = false;
        }
        self.reveal();
        cx.notify();
    }

    fn previous(&self, at: usize) -> usize {
        self.text[..at]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map(|c| at + c.len_utf8())
            .unwrap_or(at)
    }

    fn word_left(&self, at: usize) -> usize {
        let before = &self.text[..at];
        let trimmed = before.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_');
        trimmed
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| i + trimmed[i..].chars().next().map_or(1, char::len_utf8))
            .unwrap_or(0)
    }

    fn word_right(&self, at: usize) -> usize {
        let after = &self.text[at..];
        let skip = after.len()
            - after
                .trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .len();
        let rest = &after[skip..];
        at + skip
            + rest
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(rest.len())
    }

    /// The position `rows` lines away at the same shown column.
    fn vertical(&self, rows: isize) -> usize {
        let n = self.line_of(self.cursor());
        let r = self.line_range(n);
        let col = to_shown(&self.text[r.clone()], self.cursor() - r.start);
        let m = (n as isize + rows).clamp(0, self.starts.len() as isize - 1) as usize;
        if m == n {
            return if rows < 0 { 0 } else { self.text.len() };
        }
        let r = self.line_range(m);
        r.start + from_shown(&self.text[r.clone()], col)
    }

    fn snapshot(&mut self, grouped: bool) {
        let fresh = self.last_edit.is_none_or(|t| t.elapsed() > UNDO_GROUP);
        if !grouped || fresh {
            self.undo.push((self.text.clone(), self.selected.clone()));
            if self.undo.len() > UNDO_DEPTH {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.last_edit = Some(Instant::now());
    }

    fn replace(&mut self, r: Range<usize>, s: &str, cx: &mut Context<Self>) {
        self.text.replace_range(r.clone(), s);
        self.reindex();
        let at = r.start + s.len();
        self.selected = at..at;
        self.reversed = false;
        self.reveal();
        cx.emit(CodeEvent::Changed);
        cx.notify();
    }

    fn insert(&mut self, s: &str, cx: &mut Context<Self>) {
        if !self.editable {
            return;
        }
        self.snapshot(s.chars().count() == 1 && s != "\n");
        let r = self.marked.take().unwrap_or(self.selected.clone());
        self.replace(r, s, cx);
    }

    fn erase(&mut self, to: usize, cx: &mut Context<Self>) {
        if !self.editable {
            return;
        }
        let r = if self.selected.is_empty() {
            let c = self.cursor();
            c.min(to)..c.max(to)
        } else {
            self.selected.clone()
        };
        if r.is_empty() {
            return;
        }
        self.snapshot(true);
        self.replace(r, "", cx);
    }

    fn restore(&mut self, back: bool, cx: &mut Context<Self>) {
        let (from, to) = if back {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        if let Some((text, sel)) = from.pop() {
            to.push((
                std::mem::replace(&mut self.text, text),
                self.selected.clone(),
            ));
            self.reindex();
            self.selected = sel.start.min(self.text.len())..sel.end.min(self.text.len());
            self.last_edit = None;
            self.reveal();
            cx.emit(CodeEvent::Changed);
            cx.notify();
        }
    }

    fn copy(&self, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text[self.selected.clone()].to_string(),
            ));
        }
    }

    /// The keys the text owns. Printable keys come through the input
    /// handler, so an input method works.
    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = k.modifiers;
        let ctrl = m.control || m.platform;
        let shift = m.shift;
        let c = self.cursor();
        let handled = match (k.key.as_str(), ctrl, m.alt) {
            ("s", true, false) => {
                cx.emit(CodeEvent::Save);
                true
            }
            ("escape", false, false) => {
                cx.emit(CodeEvent::Escape);
                true
            }
            ("c", true, false) | ("insert", true, false) => {
                self.copy(cx);
                true
            }
            ("a", true, false) => {
                self.selected = 0..self.text.len();
                self.reversed = false;
                cx.notify();
                true
            }
            ("x", true, false) if self.editable => {
                self.copy(cx);
                self.erase(c, cx);
                true
            }
            ("v", true, false) if self.editable => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.snapshot(false);
                    let r = self.selected.clone();
                    self.replace(r, &t.replace("\r\n", "\n"), cx);
                }
                true
            }
            ("insert", false, false) if shift && self.editable => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.snapshot(false);
                    let r = self.selected.clone();
                    self.replace(r, &t.replace("\r\n", "\n"), cx);
                }
                true
            }
            ("z", true, false) => {
                self.restore(!shift, cx);
                true
            }
            ("y", true, false) => {
                self.restore(false, cx);
                true
            }
            ("tab", false, false) if self.editable && !shift => {
                self.insert(INDENT, cx);
                true
            }
            ("enter", false, false) if self.editable && !shift => {
                self.insert("\n", cx);
                true
            }
            ("backspace", _, false) => {
                let to = if ctrl {
                    self.word_left(c)
                } else {
                    self.previous(c)
                };
                self.erase(to, cx);
                true
            }
            ("delete", _, false) => {
                let to = if ctrl {
                    self.word_right(c)
                } else {
                    self.next(c)
                };
                self.erase(to, cx);
                true
            }
            ("left", _, false) => {
                let at = if !shift && !self.selected.is_empty() {
                    self.selected.start
                } else if ctrl {
                    self.word_left(c)
                } else {
                    self.previous(c)
                };
                self.move_to(at, shift, cx);
                true
            }
            ("right", _, false) => {
                let at = if !shift && !self.selected.is_empty() {
                    self.selected.end
                } else if ctrl {
                    self.word_right(c)
                } else {
                    self.next(c)
                };
                self.move_to(at, shift, cx);
                true
            }
            ("up", false, false) | ("down", false, false) if !self.editable && !shift => {
                let by = if k.key == "up" { -1. } else { 1. };
                self.scroll_lines(by, cx);
                true
            }
            ("up", false, false) => {
                let at = self.vertical(-1);
                self.move_to(at, shift, cx);
                true
            }
            ("down", false, false) => {
                let at = self.vertical(1);
                self.move_to(at, shift, cx);
                true
            }
            ("pageup", false, false) | ("pagedown", false, false) => {
                let rows = self.rows_in_view().saturating_sub(1).max(1) as isize;
                let rows = if k.key == "pageup" { -rows } else { rows };
                if self.editable {
                    let at = self.vertical(rows);
                    self.move_to(at, shift, cx);
                } else {
                    self.scroll_lines(rows as f32, cx);
                }
                true
            }
            ("home", _, false) => {
                let at = if ctrl {
                    0
                } else {
                    let r = self.line_range(self.line_of(c));
                    // First Home goes to the indent, the next to the edge.
                    let indent = r.start
                        + (self.text[r.clone()].len() - self.text[r.clone()].trim_start().len());
                    if c == indent { r.start } else { indent }
                };
                if !self.editable && ctrl {
                    self.top = 0.;
                }
                self.move_to(at, shift, cx);
                true
            }
            ("end", _, false) => {
                let at = if ctrl {
                    self.text.len()
                } else {
                    self.line_range(self.line_of(c)).end
                };
                self.move_to(at, shift, cx);
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
            window.prevent_default();
        }
    }

    fn scroll_lines(&mut self, by: f32, cx: &mut Context<Self>) {
        let max = (self.starts.len() as f32 - self.rows_in_view() as f32).max(0.);
        self.top = (self.top + by).clamp(0., max);
        cx.notify();
    }

    fn wheel(&mut self, e: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let d = e.delta.pixel_delta(line_height());
        let (dx, dy) = if e.modifiers.shift && d.x == px(0.) {
            (d.y, px(0.))
        } else {
            (d.x, d.y)
        };
        self.scroll_lines(-(dy / line_height()), cx);
        self.left = (self.left - dx).max(px(0.));
        cx.stop_propagation();
        cx.notify();
    }

    fn index_at(&self, p: Point<Pixels>, window: &mut Window) -> usize {
        let Some(b) = self.area else { return 0 };
        let row = ((p.y - b.origin.y) / line_height() + self.top)
            .floor()
            .max(0.) as usize;
        let n = row.min(self.starts.len() - 1);
        let r = self.line_range(n);
        let line = &self.text[r.clone()];
        let shaped = shape(window, &expand(line), MEASURE, &[]);
        let x = (p.x - b.origin.x + self.left).max(px(0.));
        r.start + from_shown(line, shaped.closest_index_for_x(x))
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let at = self.index_at(e.position, window);
        if e.click_count >= 2 {
            let s = self.word_left(self.next(at));
            let end = self.word_right(at);
            self.selected = s.min(end)..end;
            self.reversed = false;
            cx.notify();
            return;
        }
        self.selecting = true;
        self.move_to(at, e.modifiers.shift, cx);
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && e.pressed_button == Some(MouseButton::Left) {
            let at = self.index_at(e.position, window);
            self.move_to(at, true, cx);
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
}

/// Shaping only to measure: the colour is never drawn.
const MEASURE: gpui::Hsla = gpui::Hsla {
    h: 0.,
    s: 0.,
    l: 1.,
    a: 1.,
};

/// Shapes `text` in `color`, its `coloured` stretches (in order, apart) in
/// theirs.
fn shape(
    window: &mut Window,
    text: &str,
    color: Hsla,
    coloured: &[(Range<usize>, Hsla)],
) -> ShapedLine {
    let text: SharedString = if text.is_empty() {
        " ".into()
    } else {
        text.to_string().into()
    };
    let run = |len: usize, color: Hsla| TextRun {
        len,
        font: gpui::font(FAMILY),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = vec![];
    let mut at = 0;
    for (r, c) in coloured {
        if r.is_empty() || r.start < at || r.end > text.len() {
            continue;
        }
        if r.start > at {
            runs.push(run(r.start - at, color));
        }
        runs.push(run(r.len(), *c));
        at = r.end;
    }
    if at < text.len() || runs.is_empty() {
        runs.push(run(text.len() - at, color));
    }
    window
        .text_system()
        .shape_line(text, px(size::TEXT), &runs, None)
}

/// A token's colour in the theme.
fn token_colour(t: &Theme, token: Token) -> Hsla {
    match token {
        Token::Comment => t.dim,
        Token::String => rgb(t.ansi[2]),
        Token::Number => rgb(t.ansi[3]),
        Token::Keyword => t.accent,
    }
}

impl EntityInputHandler for Code {
    fn text_for_range(
        &mut self,
        r: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let r = self.utf16_to(r.start)..self.utf16_to(r.end);
        actual.replace(self.to_utf16(r.start)..self.to_utf16(r.end));
        Some(self.text[r].to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.to_utf16(self.selected.start)..self.to_utf16(self.selected.end),
            reversed: self.reversed,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|r| self.to_utf16(r.start)..self.to_utf16(r.end))
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
        if !self.editable {
            return;
        }
        if let Some(r) = r {
            self.marked = None;
            self.selected = self.utf16_to(r.start)..self.utf16_to(r.end);
        }
        let text = if text == "\r" { "\n" } else { text };
        self.insert(text, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        _sel: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.editable {
            return;
        }
        let r = r
            .map(|r| self.utf16_to(r.start)..self.utf16_to(r.end))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        self.text.replace_range(r.clone(), text);
        self.reindex();
        self.marked = (!text.is_empty()).then(|| r.start..r.start + text.len());
        let at = r.start + text.len();
        self.selected = at..at;
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        r: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let b = self.area?;
        let at = self.utf16_to(r.start);
        let n = self.line_of(at);
        let lr = self.line_range(n);
        let line = &self.text[lr.clone()];
        let shaped = shape(window, &expand(line), MEASURE, &[]);
        let x = shaped.x_for_index(to_shown(line, at - lr.start));
        let y = (n as f32 - self.top) * line_height();
        Some(Bounds::new(
            point(b.origin.x + x - self.left, b.origin.y + y),
            sz(px(2.), line_height()),
        ))
    }
    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let i = self.index_at(p, window);
        Some(self.to_utf16(i))
    }
}

struct CodeElement {
    code: Entity<Code>,
}

struct Painted {
    gutter: Vec<(Point<Pixels>, ShapedLine)>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    quads: Vec<gpui::PaintQuad>,
    caret: Option<gpui::PaintQuad>,
    area: Bounds<Pixels>,
    char_w: Pixels,
}

impl IntoElement for CodeElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for CodeElement {
    type RequestLayoutState = ();
    type PrepaintState = Painted;
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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
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
    ) -> Painted {
        let t = theme(cx).clone();
        let c = self.code.read(cx);
        let lh = line_height();
        let digits = c.starts.len().to_string().len().max(3);
        let char_w = shape(window, "0", t.text, &[]).width;
        let gutter_w = char_w * digits as f32 + px(24.);
        let area = Bounds::new(
            point(bounds.origin.x + gutter_w, bounds.origin.y),
            sz(bounds.size.width - gutter_w, bounds.size.height),
        );
        let first = c.top.max(0.) as usize;
        let offset = (c.top - first as f32) * lh;
        let rows = (bounds.size.height / lh).ceil() as usize + 1;
        let caret_at = c.cursor();
        let mut gutter = vec![];
        let mut lines = vec![];
        let mut quads = vec![];
        let mut caret = None;
        let caret_line = c.line_of(caret_at);
        for n in first..(first + rows).min(c.starts.len()) {
            let y = bounds.origin.y + lh * (n - first) as f32 - offset;
            let r = c.line_range(n);
            let line = &c.text[r.clone()];
            let shown = expand(line);
            let coloured: Vec<_> = c
                .tokens(n)
                .into_iter()
                .map(|(s, token)| {
                    (
                        to_shown(line, s.start)..to_shown(line, s.end),
                        token_colour(&t, token),
                    )
                })
                .collect();
            let shaped = shape(window, &shown, t.text, &coloured);
            let num = shape(
                window,
                &format!("{:>digits$}", n + 1),
                if n == caret_line && c.editable {
                    t.text2
                } else {
                    t.dim
                },
                &[],
            );
            gutter.push((point(bounds.origin.x + px(10.), y), num));
            let x0 = area.origin.x - c.left;
            // The selection's band on this line.
            let s = c.selected.start.max(r.start);
            let e = c.selected.end.min(r.end);
            if !c.selected.is_empty()
                && s <= e
                && c.selected.start <= r.end
                && c.selected.end >= r.start
            {
                let xs = shaped.x_for_index(to_shown(line, s - r.start));
                let mut xe = shaped.x_for_index(to_shown(line, e - r.start));
                if c.selected.end > r.end {
                    xe += char_w;
                }
                quads.push(fill(
                    Bounds::new(point(x0 + xs, y), sz((xe - xs).max(px(2.)), lh)),
                    alpha(t.accent, 0.3),
                ));
            }
            if c.editable && n == caret_line {
                let x = shaped.x_for_index(to_shown(line, caret_at - r.start));
                caret = Some(fill(
                    Bounds::new(point(x0 + x, y + px(2.)), sz(px(1.5), lh - px(4.))),
                    t.accent,
                ));
                quads.insert(
                    0,
                    fill(
                        Bounds::new(point(bounds.origin.x, y), sz(bounds.size.width, lh)),
                        alpha(t.text, 0.04),
                    ),
                );
            }
            lines.push((point(x0, y), shaped));
        }
        Painted {
            gutter,
            lines,
            quads,
            caret,
            area,
            char_w,
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        p: &mut Painted,
        window: &mut Window,
        cx: &mut App,
    ) {
        let t = theme(cx).clone();
        let focus = self.code.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.code.clone()),
            cx,
        );
        let lh = line_height();
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            window.paint_quad(fill(
                Bounds::new(
                    bounds.origin,
                    sz(
                        p.area.origin.x - bounds.origin.x - px(8.),
                        bounds.size.height,
                    ),
                ),
                t.surface,
            ));
            for (o, l) in &p.gutter {
                let _ = l.paint(*o, lh, gpui::TextAlign::Left, None, window, cx);
            }
            window.with_content_mask(Some(gpui::ContentMask { bounds: p.area }), |window| {
                for q in p.quads.drain(..) {
                    window.paint_quad(q);
                }
                for (o, l) in &p.lines {
                    let _ = l.paint(*o, lh, gpui::TextAlign::Left, None, window, cx);
                }
                if focus.is_focused(window)
                    && let Some(c) = p.caret.take()
                {
                    window.paint_quad(c);
                }
            });
        });
        let (area, char_w) = (p.area, p.char_w);
        self.code.update(cx, |c, _| {
            c.area = Some(area);
            c.char_w = char_w;
        });
    }
}

impl Render for Code {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .id("code")
            .key_context("Code")
            .track_focus(&self.focus)
            .size_full()
            .bg(t.bg)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|c, _, _, _| c.selecting = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|c, _, _, _| c.selecting = false),
            )
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll(cx.listener(Self::wheel))
            .child(CodeElement { code: cx.entity() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_show_as_spaces_and_map_back() {
        let line = "\tab\tc";
        assert_eq!(expand(line), "    ab    c");
        assert_eq!(to_shown(line, 1), 4);
        assert_eq!(to_shown(line, 4), 10);
        assert_eq!(from_shown(line, 4), 1);
        assert_eq!(from_shown(line, 10), 4);
        assert_eq!(from_shown(line, 99), line.len());
        assert_eq!(starts_of("a\nbc\n"), vec![0, 2, 5]);
    }

    #[gpui::test]
    fn editing_by_keyboard(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(crate::theme::Theme::violet()));
        let code = cx.new(Code::new);
        code.update(cx, |c, cx| {
            c.set("one\ntwo\nthree", cx);
            c.top = 1.;
            c.editable = true;
            let at = c.starts[c.top_line()];
            c.selected = at..at;
            c.insert(INDENT, cx);
            assert_eq!(c.text(), "one\n  two\nthree");
            c.last_edit = None;
            c.move_to(c.vertical(1), false, cx);
            assert_eq!(c.line_of(c.cursor()), 2);
            c.erase(c.previous(c.cursor()), cx);
            c.insert("x", cx);
            c.restore(true, cx);
            assert_eq!(c.text(), "one\n  two\nthree", "typing undoes as a group");
            c.restore(true, cx);
            assert_eq!(c.text(), "one\ntwo\nthree");
            c.restore(false, cx);
            assert_eq!(c.text(), "one\n  two\nthree");
            c.editable = false;
            c.insert("y", cx);
            assert_eq!(c.text(), "one\n  two\nthree", "reading does not type");
        });
    }

    /// What a page that owns an editor hears: an edit, a save, an Escape.
    #[gpui::test]
    fn its_owner_hears_edits_saves_and_escape(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(|cx| cx.set_global(crate::theme::Theme::violet()));
        let (code, cx) = cx.add_window_view(|_, cx| Code::new(cx));
        let heard = Rc::new(RefCell::new(vec![]));
        let keep = heard.clone();
        cx.update(|window, cx| {
            cx.subscribe(&code, move |_, e: &CodeEvent, _| keep.borrow_mut().push(*e))
                .detach();
            code.update(cx, |c, cx| {
                c.set("draft", cx);
                c.edit(true, window, cx);
            });
            Code::focus(&code, window, cx);
        });
        cx.run_until_parked();
        cx.simulate_input("!");
        cx.simulate_keystrokes("ctrl-s escape");
        assert_eq!(code.read_with(cx, |c, _| c.text().to_string()), "!draft");
        assert_eq!(
            *heard.borrow(),
            [CodeEvent::Changed, CodeEvent::Save, CodeEvent::Escape]
        );
    }

    #[gpui::test]
    fn colours_follow_edits_across_lines(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(crate::theme::Theme::violet()));
        let code = cx.new(Code::new);
        code.update(cx, |c, cx| {
            c.set_lang(syntax::lang("main.rs"), cx);
            c.set("/* a\nfn b */ fn", cx);
            assert_eq!(
                c.tokens(1),
                vec![(0..7, Token::Comment), (8..10, Token::Keyword)]
            );
            c.editable = true;
            c.selected = 0..2;
            c.insert("", cx);
            assert_eq!(
                c.tokens(1),
                vec![(0..2, Token::Keyword), (8..10, Token::Keyword)],
                "with the comment's opener gone, the next line is code"
            );
            c.set_lang(None, cx);
            assert!(c.tokens(1).is_empty(), "a file of no language is plain");
        });
    }
}
