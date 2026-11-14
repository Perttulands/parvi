//! A press in a window keeps following the pointer wherever it goes until
//! the button comes up, as xterm follows it on the document: a selection
//! keeps extending, held to the grid's edge, and scrolls a history; a
//! program that has the mouse keeps hearing the drag, at the nearest cell.

use super::super::page::Terminals;
use crate::term::{drag, mouse};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use gpui::{Context, MouseButton, MouseMoveEvent, Pixels, Point, Window, point, px};

impl Terminals {
    /// The pointer moved anywhere in the app while a press from a window is
    /// in flight. Moves with no press are the grid's own (links, motion).
    pub fn term_drag(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((i, b)) = self.mouse_down_in {
            self.drag_report(i, b, e, cx);
            return;
        }
        let Some(i) = self.selecting else {
            return;
        };
        if e.pressed_button != Some(MouseButton::Left) {
            return;
        }
        // Reading back on the live screen, a selection runs on into the
        // docked input: it is the same screen's rows.
        if let Some((p, side)) = self.dock_point(i, e.position) {
            self.extend_selection(i, p, side, 0);
            self.end_drag();
            cx.notify();
            return;
        }
        let Some((top, bottom)) = self.view_span(i) else {
            return;
        };
        let amount = drag::scroll_amount(f32::from(e.position.y - top), f32::from(bottom - top));
        let held = point(e.position.x, e.position.y.clamp(top, bottom - px(0.5)));
        if let Some((p, side)) = self.grid_point(i, held) {
            self.extend_selection(i, p, side, amount);
        }
        self.drag_scroll = amount;
        let scrolls = amount != 0 && self.histories.contains_key(&i);
        if scrolls && self.drag_scroll_task.is_none() {
            self.drag_scroll_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(drag::SCROLL_EVERY).await;
                    let go = this.update(cx, |s, cx| s.drag_scroll_tick(i, cx));
                    if !matches!(go, Ok(true)) {
                        break;
                    }
                }
            }));
        } else if !scrolls {
            self.drag_scroll_task = None;
        }
        cx.notify();
    }

    /// The press is over: no more scrolling.
    pub fn end_drag(&mut self) {
        self.drag_scroll = 0;
        self.drag_scroll_task = None;
    }

    /// One tick of a drag held above or below a history: scroll it, and take
    /// the selection to the row that came into view. False stops the ticks.
    fn drag_scroll_tick(&mut self, i: usize, cx: &mut Context<Self>) -> bool {
        let amount = self.drag_scroll;
        let Some(h) = self.histories.get(&i).filter(|_| self.selecting == Some(i)) else {
            self.drag_scroll_task = None;
            return false;
        };
        if amount == 0 {
            self.drag_scroll_task = None;
            return false;
        }
        let screen = h.screen.clone();
        let (top, offset, rows, cols) = {
            let mut s = screen.lock();
            let top = s.scroll_view(h.top, -amount).unwrap_or(s.top());
            (top, s.term.grid().history_size() - top, s.rows, s.cols)
        };
        if let Some(h) = self.histories.get_mut(&i) {
            h.top = top;
        }
        let row = if amount > 0 { rows - 1 } else { 0 };
        let p = GridPoint::new(Line(row as i32) - offset, Column(cols - 1));
        self.extend_selection(i, p, Side::Right, amount);
        cx.notify();
        true
    }

    /// Moves the selection's end. Past the bottom edge it takes the rest of
    /// the row, past the top edge the start of it.
    fn extend_selection(&mut self, i: usize, mut p: GridPoint, mut side: Side, amount: i32) {
        let Some((screen, _)) = self.shown_screen(i) else {
            return;
        };
        let mut s = screen.lock();
        if amount > 0 {
            p.column = Column(s.cols - 1);
            side = Side::Right;
        } else if amount < 0 {
            p.column = Column(0);
            side = Side::Left;
        }
        if let Some(sel) = s.term.selection.as_mut() {
            sel.update(p, side);
        }
    }

    /// The live row and column under a pointer at or below the docked
    /// input, when the view above is the same screen.
    fn dock_point(&self, i: usize, p: Point<Pixels>) -> Option<(GridPoint, Side)> {
        let g = self.geo.borrow()[i];
        let (dock, o, cell) = (g.dock?, g.dock_origin?, g.cell?);
        if p.y < dock.top() {
            return None;
        }
        let x = f32::from(p.x - o.x) / f32::from(cell.width);
        let y = f32::from(p.y.min(dock.bottom() - px(0.5)) - o.y) / f32::from(cell.height);
        let col = (x.max(0.) as usize).min(g.cols.saturating_sub(1));
        let row = (y.max(0.) as usize).min(g.rows.saturating_sub(1));
        let side = if x.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        Some((GridPoint::new(Line(row as i32), Column(col)), side))
    }

    /// The top and bottom of window `i`'s grid where it is on screen: a grid
    /// taller than its window is clipped at the top.
    fn view_span(&self, i: usize) -> Option<(Pixels, Pixels)> {
        let g = self.geo.borrow()[i];
        let (body, o, cell) = (g.body?, g.origin?, g.cell?);
        let top = body.top().max(o.y);
        let end = g.dock.map_or(body.bottom(), |d| d.top());
        let bottom = end.min(o.y + cell.height * g.rows as f32);
        (bottom > top).then_some((top, bottom))
    }

    /// A drag where the program has the mouse goes on being reported, at
    /// the cell nearest the pointer (`cell_at` holds it to the grid), while
    /// the program wants motion.
    fn drag_report(&mut self, i: usize, b: u8, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((mode, pane)) = self.mouse_mode(i, false) else {
            return;
        };
        if !mouse::wants(mode, true, true) {
            return;
        }
        let Some((col, row, _)) = self.cell_at(i, e.position) else {
            return;
        };
        let button = match b {
            0 => mouse::Button::Left,
            1 => mouse::Button::Middle,
            _ => mouse::Button::Right,
        };
        let mods = (e.modifiers.shift, e.modifiers.alt, e.modifiers.control);
        self.fleet.input(
            &pane.node,
            pane.pane,
            &mouse::report(mode, button, true, true, col, row, mods),
        );
        self.flush(cx);
    }
}
