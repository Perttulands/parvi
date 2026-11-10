//! The sparse-row fork must retain vt100 0.16.2's rendering semantics.

#[test]
fn deleting_a_full_colored_row_uses_default_background() {
    let mut parser = vt100::Parser::new(2, 8, 0);
    parser.process(b"\x1b[41m\x1b[2J\x1b[HX\x1b[H\x1b[8P");
    for col in 0..8 {
        assert_eq!(
            parser.screen().cell(0, col).unwrap().bgcolor(),
            vt100::Color::Default
        );
    }
}

#[test]
fn sparse_rows_match_upstream() {
    let mut actual = vt100::Parser::new(4, 8, 0);
    let mut upstream = vt100_upstream::Parser::new(4, 8, 0);
    let sequences: &[&[u8]] = &[
        b"\x1b[41m\x1b[2J",
        b"\x1b[HX\x1b[H\x1b[8P",
        b"\x1b[0mabc\x1b[2@Z",
        b"\x1b[1;2H\x1b[3P",
        b"\x1b[42m\x1b[2X",
        b"\x1b[3;2H\x1b[1K",
        b"\x1b[4;1Hone\r\ntwo\r\nthree",
        b"\x1b[2;4H\x1b[1L\x1b[1M",
        b"\x1b[?1049h\x1b[44m\x1b[2Jalt\x1b[?1049l",
        b"\x1b[1;1H\x1b[2J",
    ];
    for (index, sequence) in sequences.iter().enumerate() {
        actual.process(sequence);
        upstream.process(sequence);
        assert_screens(&actual, &upstream, index);
        if index == 4 {
            actual.screen_mut().set_size(5, 11);
            upstream.screen_mut().set_size(5, 11);
            assert_screens(&actual, &upstream, index);
        }
        if index == 7 {
            actual.screen_mut().set_size(3, 6);
            upstream.screen_mut().set_size(3, 6);
            assert_screens(&actual, &upstream, index);
        }
    }
}

fn assert_screens(actual: &vt100::Parser, upstream: &vt100_upstream::Parser, step: usize) {
    let a = actual.screen();
    let b = upstream.screen();
    assert_eq!(
        a.contents_formatted(),
        b.contents_formatted(),
        "step {step}"
    );
    assert_eq!(a.cursor_position(), b.cursor_position(), "step {step}");
    assert_eq!(a.size(), b.size());
    let (rows, cols) = a.size();
    for row in 0..rows {
        for col in 0..cols {
            let x = a.cell(row, col).unwrap();
            let y = b.cell(row, col).unwrap();
            assert_eq!(x.contents(), y.contents(), "step {step}, {row},{col}");
            assert_eq!(
                format!("{:?}", x.fgcolor()),
                format!("{:?}", y.fgcolor()),
                "step {step}, {row},{col}"
            );
            assert_eq!(
                format!("{:?}", x.bgcolor()),
                format!("{:?}", y.bgcolor()),
                "step {step}, {row},{col}"
            );
            assert_eq!(
                (
                    x.bold(),
                    x.dim(),
                    x.italic(),
                    x.underline(),
                    x.inverse(),
                    x.is_wide(),
                    x.is_wide_continuation()
                ),
                (
                    y.bold(),
                    y.dim(),
                    y.italic(),
                    y.underline(),
                    y.inverse(),
                    y.is_wide(),
                    y.is_wide_continuation()
                ),
                "step {step}, {row},{col}"
            );
        }
    }
}
