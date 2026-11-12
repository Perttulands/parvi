//! Recover the best available drawing attributes after a screen capture.

use vt100::{Color, Screen};

/// tmux exposes the captured cells, but not the pane's current SGR state.
/// The cell before the cursor is the best evidence of the active rendition.
/// This cannot detect an SGR change made after that cell was written, or
/// infer attributes at column zero; those cases fall back to this heuristic
/// (default attributes when there is no preceding cell).
///
/// The background is not carried: erases paint with it, so a wrong guess
/// floods the screen. Codex's composer sits on a gray band and its redraw
/// after a resize erases the whole screen before setting any colour, which
/// left Parvi gray where tmux was not.
pub(crate) fn before_cursor(screen: &Screen, row: u16, col: u16) -> Vec<u8> {
    let Some(mut col) = col.checked_sub(1) else {
        return b"\x1b[0m".to_vec();
    };
    let Some(mut cell) = screen.cell(row, col) else {
        return b"\x1b[0m".to_vec();
    };
    if cell.is_wide_continuation()
        && let Some(previous) = col.checked_sub(1)
    {
        col = previous;
        if let Some(leading) = screen.cell(row, col) {
            cell = leading;
        }
    }
    let mut params = vec!["0".to_owned()];
    for (enabled, code) in [
        (cell.bold(), 1),
        (cell.dim(), 2),
        (cell.italic(), 3),
        (cell.underline(), 4),
        (cell.inverse(), 7),
    ] {
        if enabled {
            params.push(code.to_string());
        }
    }
    color(&mut params, 38, cell.fgcolor());
    format!("\x1b[{}m", params.join(";")).into_bytes()
}

fn color(params: &mut Vec<String>, target: u8, color: Color) {
    match color {
        Color::Default => {}
        Color::Idx(index) => params.extend([target.to_string(), "5".into(), index.to_string()]),
        Color::Rgb(r, g, b) => params.extend([
            target.to_string(),
            "2".into(),
            r.to_string(),
            g.to_string(),
            b.to_string(),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_attributes_from_a_wide_cell_and_defaults_at_column_zero() {
        let mut source = vt100::Parser::new(2, 20, 0);
        source.process("\x1b[2;3;4;7;38;2;1;2;3;48;5;42m界".as_bytes());
        let mut next = vt100::Parser::new(2, 20, 0);
        next.process(&before_cursor(source.screen(), 0, 2));
        next.process(b"x");
        let cell = next.screen().cell(0, 0).unwrap();
        assert!(cell.dim() && cell.italic() && cell.underline() && cell.inverse());
        assert_eq!(cell.fgcolor(), Color::Rgb(1, 2, 3));
        next.process(&before_cursor(source.screen(), 0, 0));
        next.process(b"y");
        assert!(!next.screen().cell(0, 1).unwrap().dim());
    }

    #[test]
    fn an_erase_after_a_cursor_in_a_coloured_band_leaves_the_screen_plain() {
        // Codex after a resize: cursor in its gray composer, then a full
        // erase before any colour is set.
        let mut source = vt100::Parser::new(4, 10, 0);
        source.process(b"\x1b[3;1H\x1b[48;2;30;30;30m> \x1b[K\x1b[0m");
        let mut next = vt100::Parser::new(4, 10, 0);
        next.process(&before_cursor(source.screen(), 2, 2));
        next.process(b"\x1b[1;1H\x1b[J");
        assert_eq!(next.screen().cell(1, 5).unwrap().bgcolor(), Color::Default);
    }
}
