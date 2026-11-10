//! One pane's terminal: alacritty_terminal's grid and parser. The node sends
//! a whole screen, then the bytes the pane writes; a history is the pane's
//! lines read once, shown read-only.

use super::clusters::Clusters;
use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;

/// Responses the terminal would write back (device attributes and the
/// like) are dropped: tmux is the pane's terminal and has answered already.
#[derive(Clone, Copy)]
pub struct Quiet;
impl EventListener for Quiet {
    fn send_event(&self, _: TermEvent) {}
}

pub struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Screen {
    pub term: Term<Quiet>,
    parser: Processor,
    pub cols: usize,
    pub rows: usize,
    /// Set when bytes arrive, cleared when painted; one wake per paint.
    pub dirty: bool,
    /// A screen frame has arrived since subscribing.
    pub ready: bool,
}

fn config(history: usize) -> Config {
    Config {
        scrolling_history: history,
        ..Config::default()
    }
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self::with_history(cols, rows, 0)
    }

    fn with_history(cols: usize, rows: usize, history: usize) -> Self {
        let (cols, rows) = (cols.max(2), rows.max(1));
        Self {
            term: Term::new(config(history), &Size { cols, rows }, Quiet),
            parser: Processor::new(),
            cols,
            rows,
            dirty: false,
            ready: false,
        }
    }

    /// A screen frame: start over at this grid.
    pub fn reset(&mut self, cols: usize, rows: usize) {
        *self = Self {
            dirty: self.dirty,
            ..Self::new(cols, rows)
        };
        self.ready = true;
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut Clusters(&mut self.term), bytes);
    }

    /// A pane's history as the node reads it (lines oldest first, wrapped
    /// lines joined, colours as SGR), at the pane's grid, scrolled so `line`
    /// (counted from the oldest) is in view, or to the bottom.
    pub fn history(text: &str, cols: usize, rows: usize, line: Option<usize>) -> Self {
        let lines = text.split('\n').count();
        let mut s = Self::with_history(cols, rows, lines + rows);
        s.ready = true;
        let mut bytes = Vec::with_capacity(text.len() + lines);
        for (i, l) in text.split('\n').enumerate() {
            if i > 0 {
                bytes.extend_from_slice(b"\x1b[0m\r\n");
            }
            bytes.extend_from_slice(l.as_bytes());
        }
        s.feed(&bytes);
        // Park the cursor out of the way: a history has no cursor.
        s.feed(b"\x1b[0m\x1b[?25l");
        if let Some(line) = line {
            let total = s.term.grid().history_size() + rows;
            // Display offset counts back from the bottom; centre the line.
            let from_bottom = total.saturating_sub(line + 1);
            let offset = from_bottom.saturating_sub(rows / 2);
            s.term.scroll_display(Scroll::Delta(offset as i32));
        }
        s
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn scrolled_back(&self) -> bool {
        self.term.grid().display_offset() != 0
    }

    /// How many rows from the top must stay in view when the grid is taller
    /// than its box: down to the cursor on the live screen, all of them when
    /// reading back.
    pub fn rows_to_show(&self) -> usize {
        if self.scrolled_back() {
            return self.rows;
        }
        let cursor = self.term.grid().cursor.point.line.0.max(0) as usize;
        (cursor + 1).min(self.rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Column, Line, Point};

    fn row_text(s: &Screen, row: i32) -> String {
        let grid = s.term.grid();
        let offset = grid.display_offset() as i32;
        (0..s.cols)
            .map(|c| grid[Point::new(Line(row - offset), Column(c))].c)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn a_screen_frame_starts_over_at_its_grid() {
        let mut s = Screen::new(10, 3);
        s.feed(b"old");
        s.reset(20, 4);
        s.feed(b"new");
        assert_eq!((s.cols, s.rows), (20, 4));
        assert_eq!(row_text(&s, 0), "new");
    }

    #[test]
    fn a_history_opens_at_the_line_asked_for() {
        let text: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
        let s = Screen::history(&text.join("\n"), 20, 10, Some(40));
        let shown: Vec<String> = (0..10).map(|r| row_text(&s, r)).collect();
        assert!(shown.contains(&"line 40".to_string()), "{shown:?}");
        let bottom = Screen::history(&text.join("\n"), 20, 10, None);
        assert_eq!(row_text(&bottom, 9), "line 99");
    }
}
