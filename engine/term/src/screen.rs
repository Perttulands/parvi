//! Atomic, styled view of a visible terminal screen.

pub use vt100::Color;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub width: u8,
    pub foreground: Color,
    pub background: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub inverse: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub row: u16,
    pub column: u16,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub rows: Vec<Vec<Cell>>,
    pub cursor: Cursor,
}

impl Screen {
    pub(crate) fn from_vt(screen: &vt100::Screen) -> Self {
        let (rows, cols) = screen.size();
        let rows = (0..rows)
            .map(|row| {
                (0..cols)
                    .map(|col| {
                        let cell = screen.cell(row, col).expect("cell within screen");
                        Cell {
                            text: cell.contents().to_owned(),
                            width: if cell.is_wide_continuation() {
                                0
                            } else if cell.is_wide() {
                                2
                            } else {
                                1
                            },
                            foreground: cell.fgcolor(),
                            background: cell.bgcolor(),
                            bold: cell.bold(),
                            dim: cell.dim(),
                            italic: cell.italic(),
                            inverse: cell.inverse(),
                        }
                    })
                    .collect()
            })
            .collect();
        let (row, column) = screen.cursor_position();
        Self {
            rows,
            cursor: Cursor {
                row,
                column,
                visible: !screen.hide_cursor(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Screen;

    #[test]
    fn reads_dim_and_normal_cells_and_cursor() {
        let mut parser = vt100::Parser::new(2, 12, 0);
        parser.process(b"\x1b[2mghost\x1b[22mtyped");
        let screen = Screen::from_vt(parser.screen());
        assert_eq!(screen.rows[0][0].text, "g");
        assert!(screen.rows[0][0].dim);
        assert_eq!(screen.rows[0][5].text, "t");
        assert!(!screen.rows[0][5].dim);
        assert_eq!((screen.cursor.row, screen.cursor.column), (0, 10));
        assert!(screen.cursor.visible);
    }

    #[test]
    fn blank_rows_keep_erase_attributes_through_edits_and_resize() {
        let mut parser = vt100::Parser::new(3, 8, 0);
        parser.process(b"\x1b[41m\x1b[2J\x1b[2;3HX\x1b[2@Y");
        parser.screen_mut().set_size(4, 10);
        let screen = parser.screen();
        assert_eq!(screen.cell(0, 0).unwrap().bgcolor(), vt100::Color::Idx(1));
        assert_eq!(screen.cell(1, 2).unwrap().contents(), "X");
        assert_eq!(screen.cell(1, 3).unwrap().contents(), "Y");
        assert_eq!(screen.cell(1, 7).unwrap().bgcolor(), vt100::Color::Idx(1));
        assert_eq!(screen.cell(1, 8).unwrap().bgcolor(), vt100::Color::Default);
        let mut restored = vt100::Parser::new(4, 10, 0);
        restored.process(&screen.state_formatted());
        for row in 0..4 {
            for col in 0..10 {
                assert_eq!(screen.cell(row, col), restored.screen().cell(row, col));
            }
        }
    }
}
