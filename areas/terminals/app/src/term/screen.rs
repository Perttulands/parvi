//! One pane's terminal: alacritty_terminal's grid and parser. The node sends
//! a whole screen, then the bytes the pane writes. Reading back, the pane's
//! lines as the node read them are grown above its live screen, so output
//! goes on arriving below them; on the alternate screen a history is the
//! lines read once, shown read-only.

use super::clusters::Clusters;
use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

/// The live rows the dock shows below a history when the program has no
/// input block of its own (a shell): its prompt and the output just above.
const SHELL_TAIL: usize = 3;

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
    /// Each captured logical line's first physical row, oldest first.
    history_lines: Vec<usize>,
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
            history_lines: Vec::new(),
        }
    }

    /// A screen frame: start over at this grid, without lines grown above.
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
        // A joined tmux line may wrap to many physical rows in this grid.
        let capacity = lines
            .saturating_add(rows)
            .saturating_add(text.len().saturating_mul(4));
        let mut s = Self::with_history(cols, rows, capacity);
        s.ready = true;
        for (i, l) in text.split('\n').enumerate() {
            if i > 0 {
                s.feed(b"\x1b[0m\r\n");
            }
            let grid = s.term.grid();
            s.history_lines
                .push(grid.history_size() + grid.cursor.point.line.0.max(0) as usize);
            s.feed(l.as_bytes());
        }
        // Park the cursor out of the way: a history has no cursor.
        s.feed(b"\x1b[0m\x1b[?25l");
        if let Some(line) = line.and_then(|line| s.history_lines.get(line).copied()) {
            let total = s.term.grid().history_size() + rows;
            // Display offset counts back from the bottom; centre the line.
            let from_bottom = total.saturating_sub(line + 1);
            let offset = from_bottom.saturating_sub(rows / 2);
            s.term.scroll_display(Scroll::Delta(offset as i32));
        }
        s
    }

    /// Reading back on the live screen: the pane's lines as the node read
    /// them (`history`'s text) above the screen as it is now, which goes on
    /// taking output and keeps its modes, cursor and scroll region. False on
    /// the alternate screen, whose program has no lines above it.
    pub fn grow_history(&mut self, text: &str) -> bool {
        if self.mode().contains(TermMode::ALT_SCREEN) {
            return false;
        }
        // tmux ends each line it captures with a newline; one more row
        // would put the screen's top line in the history as well.
        let text = text.strip_suffix('\n').unwrap_or(text);
        let mut read = Self::history(text, self.cols, self.rows, None);
        {
            let live = self.term.grid();
            let grid = read.term.grid_mut();
            for row in 0..self.rows as i32 {
                grid[Line(row)] = live[Line(row)].clone();
            }
            grid.cursor = live.cursor.clone();
            grid.saved_cursor = live.saved_cursor.clone();
        }
        std::mem::swap(self.term.grid_mut(), read.term.grid_mut());
        self.term.selection = None;
        self.history_lines = read.history_lines;
        true
    }

    /// Lines grown above the live screen are there to read: a screen frame
    /// (a resize, or output the app fell behind on) starts over without
    /// them, and a program that takes the alternate screen hides them.
    pub fn has_history(&self) -> bool {
        !self.history_lines.is_empty() && !self.mode().contains(TermMode::ALT_SCREEN)
    }

    /// No one reads back any more: the grown lines go, from the main screen
    /// also while a program has the alternate one.
    pub fn drop_history(&mut self) {
        self.term.set_options(config(0));
        self.history_lines.clear();
    }

    /// The row a reader's view starts at, counted from the oldest line. It
    /// stays put as output arrives below.
    pub fn top(&self) -> usize {
        let grid = self.term.grid();
        grid.history_size() - grid.display_offset()
    }

    /// Shows the view starting at row `top`, for one reader's paint or
    /// scroll; `live_view` puts it back, so the screen reads as live to
    /// everyone else.
    pub fn view(&mut self, top: usize) {
        let grid = self.term.grid();
        let offset = grid.history_size().saturating_sub(top) as i32;
        let now = grid.display_offset() as i32;
        self.term.scroll_display(Scroll::Delta(offset - now));
    }

    pub fn live_view(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    /// Scrolls a reader's view from `top` by `n` lines (up is positive):
    /// where it starts now, or `None` at the live end.
    pub fn scroll_view(&mut self, top: usize, n: i32) -> Option<usize> {
        self.view(top);
        self.term.scroll_display(Scroll::Delta(n));
        let at = self.scrolled_back().then(|| self.top());
        self.live_view();
        at
    }

    pub fn history_rows(&self, line: usize) -> Option<(usize, usize)> {
        let start = *self.history_lines.get(line)?;
        let grid = self.term.grid();
        let end = self
            .history_lines
            .get(line + 1)
            .copied()
            .map(|next| next.saturating_sub(1))
            .unwrap_or(grid.history_size() + grid.cursor.point.line.0.max(0) as usize);
        Some((start, end.max(start)))
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn scrolled_back(&self) -> bool {
        self.term.grid().display_offset() != 0
    }

    /// The lines on screen, as read, each without its trailing blanks.
    pub fn text(&self) -> String {
        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;
        (0..self.rows as i32)
            .map(|row| {
                (0..self.cols)
                    .map(|c| grid[Point::new(Line(row - offset), Column(c))].c)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
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

    /// The live screen's rows a program takes input in, kept in view below
    /// a history: the block the cursor is in, from the blank row above it
    /// (Claude Code's rule, Codex's shaded composer) down to the last row
    /// with anything on it. A block with no blank row above it within
    /// `most` rows (a shell's output) gives the cursor's line and the rows
    /// just above it ([`SHELL_TAIL`]), so what a command typed while
    /// reading back printed is in sight.
    pub fn input_rows(&self, most: usize) -> std::ops::Range<usize> {
        let grid = self.term.grid();
        let rows = self.rows;
        let blank = |row: usize| {
            (0..self.cols).all(|c| {
                let cell = &grid[Point::new(Line(row as i32), Column(c))];
                matches!(cell.c, ' ' | '\0') && cell.bg == Color::Named(NamedColor::Background)
            })
        };
        let cursor = (grid.cursor.point.line.0.max(0) as usize).min(rows - 1);
        let end = (cursor..rows).rev().find(|&r| !blank(r)).unwrap_or(cursor) + 1;
        let most = most.max(1);
        let mut top = cursor;
        loop {
            if end - top > most {
                // The cursor's line, with the rows it wraps on from. A shell
                // breaks its own line at the margin, without the wrap flag,
                // and a prompt ends in a space: a row written to its last
                // column but one counts as wrapping.
                let wraps = |r: usize| {
                    let at = |c: usize| &grid[Point::new(Line(r as i32), Column(c))];
                    at(self.cols - 1).flags.contains(Flags::WRAPLINE)
                        || (self.cols - 2..self.cols).any(|c| !matches!(at(c).c, ' ' | '\0'))
                };
                // With the output above it, from where its first line
                // starts: what a command typed while reading back printed.
                let mut top = cursor.min(end.saturating_sub(SHELL_TAIL.min(most)));
                while top > 0 && end - top < most && wraps(top - 1) {
                    top -= 1;
                }
                return top..end;
            }
            if top == 0 || blank(top - 1) {
                return top..end;
            }
            top -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn live(lines: &[&str], cols: usize, rows: usize) -> Screen {
        let mut s = Screen::new(cols, rows);
        s.feed(lines.join("\r\n").as_bytes());
        s
    }

    #[test]
    fn the_input_kept_in_view_is_the_block_the_cursor_is_in() {
        // Claude Code: a rule above the prompt, a rule and a hint below it.
        let mut s = live(
            &[
                "output",
                "more",
                "",
                "\u{2500}\u{2500}\u{2500}",
                "> hi",
                "\u{2500}\u{2500}\u{2500}",
                "  ? for shortcuts",
            ],
            20,
            12,
        );
        s.feed(b"\x1b[5;5H");
        assert_eq!(s.input_rows(5), 3..7);
        // A shell: no blank row near, so the prompt's row and the output
        // just above it.
        let s = live(&["a", "b", "c", "d", "e", "f", "$ "], 20, 12);
        assert_eq!(s.input_rows(4), 4..7);
        assert_eq!(s.input_rows(2), 5..7, "never over the most");
        // ...with the rows a long prompt wraps from.
        let s = live(
            &["a", "b", "c", "d", "e", "f", "user@host:~/a/long/path$ x"],
            20,
            12,
        );
        assert_eq!(s.input_rows(4), 5..8);
        let s = live(
            &[
                "a",
                "b",
                "c",
                "d",
                "e",
                "f",
                "user@host:~/a/long/path/that/wraps/twice$ x",
            ],
            20,
            12,
        );
        assert_eq!(s.input_rows(4), 6..9, "a prompt longer than the tail");
        // A command line the tail starts inside shows whole.
        let s = live(
            &[
                "a",
                "b",
                "c",
                "d",
                "e",
                "$ echo a-long-command",
                "output",
                "$ ",
            ],
            10,
            12,
        );
        assert_eq!(s.input_rows(8), 5..10);
        // ...also when the shell broke the line itself, at its prompt's
        // last space.
        let mut s = live(
            &["a", "b", "c", "d", "e", "f", "user@host:~/a/long$ ", "x"],
            20,
            12,
        );
        s.feed(b"\x1b[8;2H");
        assert_eq!(s.input_rows(4), 5..8);
        // Codex: blank rows inside the shaded composer belong to it.
        let mut s = live(&["out", "", ""], 20, 12);
        s.feed(b"\x1b[48;5;236m \x1b[K\r\n> ask\x1b[K\r\n \x1b[K\x1b[0m\r\n\r\n  status");
        s.feed(b"\x1b[4;6H");
        assert_eq!(s.input_rows(6), 2..7);
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

    #[test]
    fn reading_back_on_the_live_screen_keeps_its_place_as_output_arrives() {
        // The pane as the node reads it: 40 lines of history, then the
        // screen; the live screen has moved on by the time it arrives.
        let text: Vec<String> = (0..50).map(|i| format!("line {i}")).collect();
        let shown: Vec<&str> = text[40..].iter().map(String::as_str).collect();
        let mut s = live(&shown, 20, 10);
        s.feed(b"\r\nline 50\r\n$ ");
        // As tmux captures it: every line ends in a newline.
        assert!(s.grow_history(&format!("{}\n", text.join("\n"))));
        assert!(s.has_history());
        // The history runs on into the screen without a line twice.
        s.view(0);
        let all: Vec<String> = (0..s.term.grid().history_size() as i32 + 10)
            .map(|r| row_text(&s, r))
            .collect();
        s.live_view();
        let at = all.iter().position(|l| l == "line 39").unwrap();
        assert_eq!(all[at + 1], "line 42", "{all:?}");
        // A reader opens three lines up; the screen still reads as live.
        let top = s.scroll_view(s.top(), 3).unwrap();
        assert_eq!(row_text(&s, 9), "$");
        s.view(top);
        let before: Vec<String> = (0..10).map(|r| row_text(&s, r)).collect();
        s.live_view();
        // Output arrives below what is being read, which stays where it is.
        s.feed(b"\r\nnew 1\r\nnew 2\r\n$ ");
        s.view(top);
        let after: Vec<String> = (0..10).map(|r| row_text(&s, r)).collect();
        assert_eq!(before, after);
        s.live_view();
        assert_eq!(row_text(&s, 9), "$");
        assert_eq!(row_text(&s, 8), "new 2");
        // The input the dock shows is the live screen's, with what was
        // printed above it, and scrolling down past it reaches the live end.
        assert_eq!(s.input_rows(3), 7..10);
        assert_eq!(s.scroll_view(top, -100), None);
        // Nobody reads back: the lines go.
        s.drop_history();
        assert!(!s.has_history());
        assert_eq!(s.term.grid().history_size(), 0);
        s.feed(b"\r\nmore");
        assert_eq!(s.term.grid().history_size(), 0);
    }

    #[test]
    fn a_screen_frame_drops_the_lines_grown_above() {
        let mut s = live(&["a", "$ "], 20, 4);
        assert!(s.grow_history("x\ny\nz\na\n$ "));
        s.reset(20, 4);
        assert!(!s.has_history());
        // The alternate screen has no lines above it to read, and hides
        // those grown before it; they go from the main screen when dropped.
        s.feed(b"\x1b[?1049h");
        assert!(!s.grow_history("x"));
        s.feed(b"\x1b[?1049l");
        assert!(s.grow_history("x\ny\nz\na\n$ "));
        s.feed(b"\x1b[?1049h");
        assert!(!s.has_history());
        s.drop_history();
        s.feed(b"\x1b[?1049l");
        assert_eq!(s.term.grid().history_size(), 0);
    }
}
