//! A pane's screen: how big it may be, how much history it keeps, and what
//! `capture-pane` reads from it.

/// Scrollback lines kept per pane. With sizes capped at [`MAX_COLS`], a
/// pane's history costs at most 2,000 x 500 cells of about 36 bytes: 36 MB,
/// and about 8.6 MB at 120 columns. It grows only as lines scroll off, so an
/// idle session costs little.
const HISTORY_LINES: usize = 2000;
const MAX_COLS: u16 = 500;
const MAX_ROWS: u16 = 200;

/// A screen of this size, clamped, with the pane's history.
pub fn new(cols: u16, rows: u16) -> vt100::Parser {
    let (cols, rows) = clamp(cols, rows);
    vt100::Parser::new(rows, cols, HISTORY_LINES)
}

/// Clamps a requested size to what the holder allows.
pub fn clamp(cols: u16, rows: u16) -> (u16, u16) {
    (cols.clamp(1, MAX_COLS), rows.clamp(1, MAX_ROWS))
}

/// The pane's text, optionally with its history, its attributes as escape
/// sequences (`-e`) and wrapped lines joined (`-J`).
pub fn capture(screen: &mut vt100::Parser, history: bool, escapes: bool, join: bool) -> String {
    let (rows, cols) = screen.screen().size();
    let line = |screen: &vt100::Screen, n: u16| -> (String, bool) {
        let text = if escapes {
            let bytes = screen
                .rows_formatted(0, cols)
                .nth(n.into())
                .unwrap_or_default();
            format!("{}\x1b[m", String::from_utf8_lossy(&bytes))
        } else {
            screen.rows(0, cols).nth(n.into()).unwrap_or_default()
        };
        (text, screen.row_wrapped(n))
    };
    let mut lines = Vec::new();
    if history {
        screen.screen_mut().set_scrollback(usize::MAX);
        let kept = screen.screen().scrollback();
        for offset in (1..=kept).rev() {
            screen.screen_mut().set_scrollback(offset);
            lines.push(line(screen.screen(), 0));
        }
        screen.screen_mut().set_scrollback(0);
    }
    lines.extend((0..rows).map(|n| line(screen.screen(), n)));
    let mut out = String::new();
    let last = lines.len().saturating_sub(1);
    for (n, (text, wrapped)) in lines.into_iter().enumerate() {
        out.push_str(&text);
        if n < last && !(join && wrapped) {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_clamped() {
        assert_eq!(clamp(0, 9999), (1, MAX_ROWS));
        assert_eq!(new(9999, 10).screen().size(), (10, MAX_COLS));
    }

    #[test]
    fn captures_history_and_joins_wrapped_lines() {
        let mut screen = new(5, 2);
        screen.process(b"one\r\ntwo\r\nabcdefgh\r\nend");
        assert_eq!(capture(&mut screen, false, false, false), "fgh\nend");
        assert_eq!(
            capture(&mut screen, true, false, false),
            "one\ntwo\nabcde\nfgh\nend"
        );
        assert_eq!(
            capture(&mut screen, true, false, true),
            "one\ntwo\nabcdefgh\nend"
        );
        assert_eq!(screen.screen().scrollback(), 0);
    }

    #[test]
    fn history_is_bounded() {
        let mut screen = new(10, 2);
        for n in 0..HISTORY_LINES + 100 {
            screen.process(format!("{n}\r\n").as_bytes());
        }
        let lines = capture(&mut screen, true, false, false).split('\n').count();
        assert_eq!(lines, HISTORY_LINES + 2);
    }
}
