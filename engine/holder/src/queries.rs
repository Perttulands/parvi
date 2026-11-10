//! Terminal queries nobody else answers.
//!
//! In ConPTY's passthrough mode the console forwards a program's queries as
//! written and answers none of them. A real terminal would, and tmux does, and
//! crossterm-based TUIs (Codex) wait for the answers. The holder answers
//! cursor position, primary device attributes and the colour queries.
//!
//! Queries are found in stream order, also when one is split across reads,
//! and each is answered with the screen as it stood at that query.

/// The queries answered, each with its reply. The cursor position reply is
/// filled in from the screen.
const QUERIES: [(&[u8], &[u8]); 5] = [
    (b"\x1b[6n", b""),
    (b"\x1b[c", b"\x1b[?1;2c"),
    (b"\x1b[0c", b"\x1b[?1;2c"),
    (b"\x1b]10;?", b"\x1b]10;rgb:cccc/cccc/cccc\x1b\\"),
    (b"\x1b]11;?", b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
];

/// Finds queries in a pane's output, carrying a partial one across reads.
#[derive(Default)]
pub struct Queries {
    /// The bytes since an ESC that are still the start of some query.
    partial: Vec<u8>,
}

impl Queries {
    /// Feeds `data` to `screen`, stopping at each query to answer it with the
    /// cursor where the query left it. Returns the replies, in order.
    pub fn answer(&mut self, screen: &mut vt100::Parser, data: &[u8]) -> Vec<u8> {
        let mut reply = Vec::new();
        let mut fed = 0;
        for (at, &byte) in data.iter().enumerate() {
            let Some(query) = self.step(byte) else {
                continue;
            };
            screen.process(&data[fed..=at]);
            fed = at + 1;
            let (_, answer) = QUERIES[query];
            if answer.is_empty() {
                let (row, col) = screen.screen().cursor_position();
                reply.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            } else {
                reply.extend_from_slice(answer);
            }
        }
        screen.process(&data[fed..]);
        reply
    }

    /// Takes one byte; returns the query it completes, if any.
    fn step(&mut self, byte: u8) -> Option<usize> {
        if byte == 0x1b {
            self.partial.clear();
        }
        if byte != 0x1b && self.partial.is_empty() {
            return None;
        }
        self.partial.push(byte);
        if let Some(query) = QUERIES.iter().position(|(q, _)| *q == self.partial) {
            self.partial.clear();
            return Some(query);
        }
        if !QUERIES.iter().any(|(q, _)| q.starts_with(&self.partial)) {
            self.partial.clear();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(chunks: &[&[u8]]) -> Vec<u8> {
        let mut screen = vt100::Parser::new(5, 20, 0);
        let mut queries = Queries::default();
        chunks
            .iter()
            .flat_map(|chunk| queries.answer(&mut screen, chunk))
            .collect()
    }

    #[test]
    fn answers_what_was_asked() {
        assert_eq!(answer(&[b"\x1b[c"]), b"\x1b[?1;2c");
        assert_eq!(
            answer(&[b"\x1b]11;?\x07"]),
            b"\x1b]11;rgb:0000/0000/0000\x1b\\"
        );
        assert!(answer(&[b"plain \x1b[1moutput\x1b[0m"]).is_empty());
    }

    #[test]
    fn a_query_split_across_reads_is_answered() {
        assert_eq!(answer(&[b"ab\x1b[", b"6", b"n"]), b"\x1b[1;3R");
        assert_eq!(
            answer(&[b"\x1b]1", b"0;?\x1b\\"]),
            b"\x1b]10;rgb:cccc/cccc/cccc\x1b\\"
        );
    }

    #[test]
    fn each_query_is_answered_where_it_stands() {
        assert_eq!(
            answer(&[b"\x1b[6nabc\x1b[6n\r\nx\x1b[6n"]),
            b"\x1b[1;1R\x1b[1;4R\x1b[2;2R"
        );
        assert_eq!(answer(&[b"\x1b[c\x1b[0c"]), b"\x1b[?1;2c\x1b[?1;2c");
    }

    #[test]
    fn the_screen_gets_every_byte() {
        let mut screen = vt100::Parser::new(5, 20, 0);
        let mut queries = Queries::default();
        queries.answer(&mut screen, b"one \x1b[6ntwo \x1b[");
        queries.answer(&mut screen, b"1mthree");
        assert_eq!(screen.screen().contents(), "one two three");
        assert!(screen.screen().bold());
    }
}
