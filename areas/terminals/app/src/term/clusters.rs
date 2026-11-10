//! Emoji sequences laid out the way tmux lays them out. alacritty gives every
//! character its own width, so `⚠️` takes one cell, `👍🏽` four and a ZWJ
//! family six; tmux (3.6) joins them into one cell of two. The node's pane is
//! tmux's grid, and an app that positions its cursor (Claude Code, Codex)
//! counts the way tmux does, so the app's grid has to agree or every line
//! with such a sequence comes out shifted. The rules here are the ones tmux
//! was measured to follow:
//!
//! - VS16 (U+FE0F) widens the character it follows to two cells.
//! - A skin-tone modifier joins the wide character it follows.
//! - After a ZWJ, a wide character joins the cell the ZWJ is on.
//! - Regional indicators join the one before them, and the pair is wide.
//!
//! Joined characters are kept as the cell's zero-width characters, so the
//! painter shapes the whole sequence as one glyph.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{
    Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink, KeyboardModes,
    KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, PrivateMode, Rgb,
    ScpCharPath, ScpUpdateMode, StandardCharset, TabulationClearMode, cursor_icon::CursorIcon,
};
use unicode_width::UnicodeWidthChar;

const VS16: char = '\u{FE0F}';
const ZWJ: char = '\u{200D}';

fn skin_tone(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

fn regional(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// The terminal, seen through tmux's rules for emoji sequences.
pub struct Clusters<'a, T>(pub &'a mut Term<T>);

/// Every handler call but `input` goes straight to the terminal. vte's
/// `Handler` gives each method an empty default, so a method missing here
/// would be dropped silently: the list is vte 0.15's whole trait.
macro_rules! forward {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(fn $name(&mut self, $($arg: $ty),*) { self.0.$name($($arg),*) })*
    };
}

impl<T: EventListener> Clusters<'_, T> {
    /// The cell the last character went into, as alacritty finds it for a
    /// zero-width character: left of the cursor, or under it when the line
    /// is full, and the first half of a wide character.
    fn last(&self) -> Option<(Line, Column)> {
        let cursor = &self.0.grid().cursor;
        let mut col = cursor.point.column.0;
        if !cursor.input_needs_wrap {
            col = col.checked_sub(1)?;
        }
        let line = cursor.point.line;
        if self.0.grid()[line][Column(col)]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
        {
            col = col.checked_sub(1)?;
        }
        Some((line, Column(col)))
    }

    /// Joins `c` to the last cell instead of giving it cells of its own.
    fn join(&mut self, (line, col): (Line, Column), c: char) {
        self.0.grid_mut()[line][col].push_zerowidth(c);
    }

    /// Makes the last cell two wide, when the line has room for it.
    fn widen(&mut self, (line, col): (Line, Column)) {
        let cols = self.0.columns();
        let grid = self.0.grid_mut();
        if grid.cursor.input_needs_wrap || col.0 + 1 >= cols {
            return;
        }
        let next = Column(col.0 + 1);
        if grid[line][next].flags.contains(Flags::WIDE_CHAR) && col.0 + 2 < cols {
            grid[line][Column(col.0 + 2)]
                .flags
                .remove(Flags::WIDE_CHAR_SPACER);
        }
        grid[line][col].flags.insert(Flags::WIDE_CHAR);
        let mut spacer: Cell = grid.cursor.template.clone();
        spacer.c = ' ';
        spacer.flags.insert(Flags::WIDE_CHAR_SPACER);
        grid[line][next] = spacer;
        if col.0 + 2 < cols {
            grid.cursor.point.column = Column(col.0 + 2);
        } else {
            grid.cursor.input_needs_wrap = true;
        }
    }
}

impl<T: EventListener> Handler for Clusters<'_, T> {
    fn input(&mut self, c: char) {
        let Some(at) = self.last() else {
            return self.0.input(c);
        };
        let cell = &self.0.grid()[at.0][at.1];
        let wide = cell.flags.contains(Flags::WIDE_CHAR);
        let tail = cell.zerowidth().and_then(|z| z.last().copied());
        let base = cell.c;
        if c == VS16 {
            self.0.input(c);
            if !wide {
                self.widen(at);
            }
        } else if (skin_tone(c) && wide) || (tail == Some(ZWJ) && c.width() == Some(2)) {
            self.join(at, c);
        } else if regional(c) && regional(base) {
            self.join(at, c);
            if !wide {
                self.widen(at);
            }
        } else {
            self.0.input(c);
        }
    }

    forward! {
        set_title(a0: Option<String>);
        set_cursor_style(a0: Option<CursorStyle>);
        set_cursor_shape(a0: CursorShape);
        goto(a0: i32, a1: usize);
        goto_line(a0: i32);
        goto_col(a0: usize);
        insert_blank(a0: usize);
        move_up(a0: usize);
        move_down(a0: usize);
        identify_terminal(a0: Option<char>);
        device_status(a0: usize);
        move_forward(a0: usize);
        move_backward(a0: usize);
        move_down_and_cr(a0: usize);
        move_up_and_cr(a0: usize);
        put_tab(a0: u16);
        backspace();
        carriage_return();
        linefeed();
        bell();
        substitute();
        newline();
        set_horizontal_tabstop();
        scroll_up(a0: usize);
        scroll_down(a0: usize);
        insert_blank_lines(a0: usize);
        delete_lines(a0: usize);
        erase_chars(a0: usize);
        delete_chars(a0: usize);
        move_backward_tabs(a0: u16);
        move_forward_tabs(a0: u16);
        save_cursor_position();
        restore_cursor_position();
        clear_line(a0: LineClearMode);
        clear_screen(a0: ClearMode);
        clear_tabs(a0: TabulationClearMode);
        set_tabs(a0: u16);
        reset_state();
        reverse_index();
        terminal_attribute(a0: Attr);
        set_mode(a0: Mode);
        unset_mode(a0: Mode);
        report_mode(a0: Mode);
        set_private_mode(a0: PrivateMode);
        unset_private_mode(a0: PrivateMode);
        report_private_mode(a0: PrivateMode);
        set_scrolling_region(a0: usize, a1: Option<usize>);
        set_keypad_application_mode();
        unset_keypad_application_mode();
        set_active_charset(a0: CharsetIndex);
        configure_charset(a0: CharsetIndex, a1: StandardCharset);
        set_color(a0: usize, a1: Rgb);
        dynamic_color_sequence(a0: String, a1: usize, a2: &str);
        reset_color(a0: usize);
        clipboard_store(a0: u8, a1: &[u8]);
        clipboard_load(a0: u8, a1: &str);
        decaln();
        push_title();
        pop_title();
        text_area_size_pixels();
        text_area_size_chars();
        set_hyperlink(a0: Option<Hyperlink>);
        set_mouse_cursor_icon(a0: CursorIcon);
        report_keyboard_mode();
        push_keyboard_mode(a0: KeyboardModes);
        pop_keyboard_modes(a0: u16);
        set_keyboard_mode(a0: KeyboardModes, a1: KeyboardModesApplyBehavior);
        set_modify_other_keys(a0: ModifyOtherKeys);
        report_modify_other_keys();
        set_scp(a0: ScpCharPath, a1: ScpUpdateMode);
    }
}

#[cfg(test)]
mod tests {
    use crate::term::screen::Screen;
    use alacritty_terminal::index::{Column, Line};

    /// Where the cursor ends up after `text`, and what the first cell holds.
    fn lay_out(text: &str) -> (usize, String) {
        let mut s = Screen::new(20, 2);
        s.feed(text.as_bytes());
        let grid = s.term.grid();
        let cell = &grid[Line(0)][Column(0)];
        let mut first = cell.c.to_string();
        first.extend(cell.zerowidth().into_iter().flatten());
        (grid.cursor.point.column.0, first)
    }

    #[test]
    fn emoji_sequences_take_the_cells_tmux_gives_them() {
        // Columns measured with tmux 3.6 (`#{cursor_x}` after printing).
        for (text, cols) in [
            ("⚠\u{FE0F}", 2),
            ("1\u{FE0F}\u{20E3}", 2),
            ("👍🏽", 2),
            ("A🏽", 3),
            ("⚠🏽", 3),
            ("👨\u{200D}👩\u{200D}👧", 2),
            ("a\u{200D}b", 2),
            ("🇫🇮", 2),
            ("🏳\u{FE0F}\u{200D}🌈", 2),
            ("🙂", 2),
            ("日本", 4),
            ("e\u{301}", 1),
            ("⏺ ⎿ ❯", 5),
        ] {
            assert_eq!(lay_out(text).0, cols, "{text}");
        }
    }

    #[test]
    fn a_joined_sequence_is_one_cell_painted_whole() {
        assert_eq!(lay_out("👍🏽|").1, "👍🏽");
        assert_eq!(
            lay_out("👨\u{200D}👩\u{200D}👧").1,
            "👨\u{200D}👩\u{200D}👧"
        );
    }

    #[test]
    fn what_follows_a_widened_character_lands_after_it() {
        let mut s = Screen::new(20, 2);
        s.feed("⚠\u{FE0F}|".as_bytes());
        assert_eq!(s.term.grid()[Line(0)][Column(2)].c, '|');
    }
}
