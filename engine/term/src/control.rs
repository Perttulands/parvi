//! The tmux control-mode protocol.
//!
//! A control-mode client (`tmux -C`) reads commands one per line on stdin and
//! writes lines on stdout. Every command's output is wrapped in a block:
//!
//! ```text
//! %begin <time> <number> <flags>
//! ...output lines...
//! %end <time> <number> <flags>      (or %error on failure)
//! ```
//!
//! Everything outside a block is a notification starting with `%`. tmux never
//! writes a notification inside a block, and it keeps blocks and pane output
//! in order, so one ordered stream of [`Event`]s is enough to know what a
//! command saw.
//!
//! `flags` is 1 for a command this client wrote on stdin and 0 for anything
//! else, such as the `attach-session` on the client's own command line. Only
//! flag-1 replies answer our commands.

/// A tmux pane id, the number in `%12`.
pub type PaneId = u32;

/// One parsed item from a control-mode client's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The reply to a command. `from_us` is false for blocks this client didn't
    /// write, like the initial attach.
    Reply {
        from_us: bool,
        ok: bool,
        lines: Vec<String>,
    },
    Notification(Notification),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notification {
    /// Bytes a pane wrote, already unescaped. `%output`, or `%extended-output`
    /// for a client attached with `pause-after`.
    Output { pane: PaneId, data: Vec<u8> },
    /// tmux stopped sending a pane's output because we fell behind
    /// (`pause-after`). It stays paused until we ask it to continue, and the
    /// output in between is lost.
    Pause { pane: PaneId },
    /// The set of sessions on the server changed.
    SessionsChanged,
    /// A session was renamed: tmux tells every control client of any
    /// session's rename, as `%session-renamed $3 name`.
    SessionRenamed { session: String, name: String },
    /// A window was added to, closed in, or changed layout in the attached
    /// session. Any of these can add, remove or resize panes.
    WindowsChanged,
    /// A client attached to, left, or switched to or from a session.
    ClientsChanged,
    /// The client is exiting, for example because its session was killed.
    Exit { reason: Option<String> },
    /// Anything else, kept by name so it shows up when debugging.
    Other { name: String },
}

/// Turns control-mode lines into events. Feed it one line at a time without
/// the trailing newline. Lines are bytes, not text: tmux escapes only control
/// characters and the backslash, so pane output can hold any other byte,
/// valid UTF-8 or not.
#[derive(Debug, Default)]
pub struct Parser {
    block: Option<Block>,
    escaped_replies: bool,
}

#[derive(Debug)]
struct Block {
    number: String,
    from_us: bool,
    lines: Vec<String>,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// A holder control stream escapes reply-body lines that start with `%`.
    pub fn holder() -> Self {
        Self {
            block: None,
            escaped_replies: true,
        }
    }

    /// Parses one line. Returns `None` while a command block is still open.
    pub fn line(&mut self, line: impl AsRef<[u8]>) -> Option<Event> {
        let bytes = line.as_ref();
        let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
        if self.block.is_none()
            && let Some(output) = output(bytes)
        {
            return Some(Event::Notification(output));
        }
        let line = &*String::from_utf8_lossy(bytes);
        if let Some(block) = &mut self.block {
            if let Some((ok, number)) = block_end(line)
                && number == block.number
            {
                let block = self.block.take().expect("block is open");
                return Some(Event::Reply {
                    from_us: block.from_us,
                    ok,
                    lines: block.lines,
                });
            }
            let body = if self.escaped_replies && line.starts_with("%%") {
                &line[1..]
            } else {
                line
            };
            block.lines.push(body.to_owned());
            return None;
        }
        if let Some(rest) = line.strip_prefix("%begin ") {
            let mut fields = rest.split(' ');
            let _time = fields.next();
            let number = fields.next().unwrap_or_default().to_owned();
            let from_us = fields.next() == Some("1");
            self.block = Some(Block {
                number,
                from_us,
                lines: Vec::new(),
            });
            return None;
        }
        Some(Event::Notification(notification(line)))
    }
}

/// Recognises `%end` and `%error` lines and returns (succeeded, number).
fn block_end(line: &str) -> Option<(bool, &str)> {
    let (ok, rest) = if let Some(rest) = line.strip_prefix("%end ") {
        (true, rest)
    } else {
        let rest = line.strip_prefix("%error ")?;
        (false, rest)
    };
    let number = rest.split(' ').nth(1)?;
    Some((ok, number))
}

/// Parses `%output %<pane> <data>` and
/// `%extended-output %<pane> <age> ... : <data>`.
fn output(line: &[u8]) -> Option<Notification> {
    let (pane, data) = if let Some(rest) = line.strip_prefix(b"%output ") {
        split_once(rest, b" ")?
    } else {
        let rest = line.strip_prefix(b"%extended-output ")?;
        let (pane, rest) = split_once(rest, b" ")?;
        let (_age, data) = split_once(rest, b" : ")?;
        (pane, data)
    };
    Some(Notification::Output {
        pane: parse_pane(std::str::from_utf8(pane).ok()?)?,
        data: unescape(data),
    })
}

fn split_once<'a>(bytes: &'a [u8], separator: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    let at = bytes
        .windows(separator.len())
        .position(|window| window == separator)?;
    Some((&bytes[..at], &bytes[at + separator.len()..]))
}

fn notification(line: &str) -> Notification {
    let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
    match name {
        "%pause" => match parse_pane(rest) {
            Some(pane) => Notification::Pause { pane },
            None => Notification::Other {
                name: name.to_owned(),
            },
        },
        "%sessions-changed" => Notification::SessionsChanged,
        "%client-session-changed" | "%client-detached" => Notification::ClientsChanged,
        "%session-renamed" => match rest.split_once(' ') {
            Some((session, name)) if session.starts_with('$') => Notification::SessionRenamed {
                session: session.to_owned(),
                name: name.to_owned(),
            },
            _ => Notification::Other {
                name: name.to_owned(),
            },
        },
        "%window-add" | "%window-close" | "%layout-change" | "%window-pane-changed" => {
            Notification::WindowsChanged
        }
        "%exit" => Notification::Exit {
            reason: (!rest.is_empty()).then(|| rest.to_owned()),
        },
        _ => Notification::Other {
            name: name.to_owned(),
        },
    }
}

/// Parses `%12` into 12.
pub fn parse_pane(field: &str) -> Option<PaneId> {
    field.strip_prefix('%')?.parse().ok()
}

/// Undoes tmux's output escaping: bytes below space and the backslash itself
/// arrive as `\ooo` octal escapes; everything else is literal.
pub fn unescape(data: impl AsRef<[u8]>) -> Vec<u8> {
    let bytes = data.as_ref();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let digits = &bytes[i + 1..i + 4];
            if digits.iter().all(|d| (b'0'..=b'7').contains(d)) {
                let value = digits
                    .iter()
                    .fold(0u16, |acc, d| acc * 8 + u16::from(d - b'0'));
                if let Ok(byte) = u8::try_from(value) {
                    out.push(byte);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Encodes bytes for `send-keys -H`, which takes each byte as a hex word.
pub fn send_keys_hex(pane: PaneId, data: &[u8]) -> String {
    let mut command = format!("send-keys -t %{pane} -H");
    for byte in data {
        command.push_str(&format!(" {byte:02x}"));
    }
    command
}

/// `text` as one double-quoted tmux argument, taken literally: no `$` or `~`
/// expansion, and control characters escaped so the command stays one line.
pub fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        match c {
            '"' | '\\' | '$' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\x7f' => {
                quoted.push_str(&format!("\\{:03o}", c as u32));
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_text_literally_on_one_line() {
        assert_eq!(
            quote("a \"q\" \\ $HOME #{x} é\nb\tc\x01\x7f"),
            r#""a \"q\" \\ \$HOME #{x} é\nb\tc\001\177""#
        );
    }

    fn feed(parser: &mut Parser, text: &str) -> Vec<Event> {
        text.lines().filter_map(|line| parser.line(line)).collect()
    }

    #[test]
    fn unescapes_octal_and_keeps_utf8() {
        assert_eq!(unescape(r"a\015\012b"), b"a\r\nb");
        assert_eq!(unescape(r"\134"), b"\\");
        assert_eq!(unescape(r"\033[1mhi"), b"\x1b[1mhi");
        assert_eq!(unescape("päivää"), "päivää".as_bytes());
    }

    #[test]
    fn leaves_malformed_escapes_alone() {
        assert_eq!(unescape(r"\9"), b"\\9");
        assert_eq!(unescape(r"end\01"), b"end\\01");
        assert_eq!(unescape(r"\777"), b"\\777");
    }

    #[test]
    fn parses_output() {
        let mut parser = Parser::new();
        assert_eq!(
            parser.line(r"%output %3 hello\015\012"),
            Some(Event::Notification(Notification::Output {
                pane: 3,
                data: b"hello\r\n".to_vec()
            }))
        );
    }

    #[test]
    fn parses_extended_output_and_pause() {
        let mut parser = Parser::new();
        assert_eq!(
            parser.line(r"%extended-output %3 1250 : a : b\015\012"),
            Some(Event::Notification(Notification::Output {
                pane: 3,
                data: b"a : b\r\n".to_vec()
            }))
        );
        assert_eq!(
            parser.line("%pause %3"),
            Some(Event::Notification(Notification::Pause { pane: 3 }))
        );
    }

    #[test]
    fn keeps_output_that_is_not_utf8() {
        let mut parser = Parser::new();
        assert_eq!(
            parser.line(b"%output %1 \xff\xfe ok"),
            Some(Event::Notification(Notification::Output {
                pane: 1,
                data: b"\xff\xfe ok".to_vec()
            }))
        );
    }

    #[test]
    fn collects_a_reply_and_tells_ours_from_the_attach() {
        let mut parser = Parser::new();
        let events = feed(
            &mut parser,
            "%begin 1700000000 260 0\n\
             %end 1700000000 260 0\n\
             %begin 1700000001 261 1\n\
             %1 80 24\n\
             %2 80 23\n\
             %end 1700000001 261 1\n",
        );
        assert_eq!(
            events,
            vec![
                Event::Reply {
                    from_us: false,
                    ok: true,
                    lines: vec![]
                },
                Event::Reply {
                    from_us: true,
                    ok: true,
                    lines: vec!["%1 80 24".into(), "%2 80 23".into()]
                },
            ]
        );
    }

    #[test]
    fn a_reply_line_that_looks_like_a_notification_stays_in_the_block() {
        let mut parser = Parser::new();
        let events = feed(
            &mut parser,
            "%begin 1 5 1\n%output %1 not really\n%end 1 4 1\n%end 1 5 1\n",
        );
        assert_eq!(
            events,
            vec![Event::Reply {
                from_us: true,
                ok: true,
                lines: vec!["%output %1 not really".into(), "%end 1 4 1".into()]
            }]
        );
    }

    #[test]
    fn holder_reply_unescapes_marker_shaped_body_without_touching_notifications() {
        let mut parser = Parser::holder();
        let events = feed(
            &mut parser,
            "%begin 1 1 1\n%%end 0 1 1\n%%error 0 1 1\n%%%literal\n%end 1 1 1\n%pause %3\n",
        );
        assert_eq!(
            events,
            vec![
                Event::Reply {
                    from_us: true,
                    ok: true,
                    lines: vec![
                        "%end 0 1 1".into(),
                        "%error 0 1 1".into(),
                        "%%literal".into()
                    ]
                },
                Event::Notification(Notification::Pause { pane: 3 }),
            ]
        );
        let events = feed(&mut Parser::new(), "%begin 1 1 1\n%%literal\n%end 1 1 1\n");
        assert_eq!(
            events,
            vec![Event::Reply {
                from_us: true,
                ok: true,
                lines: vec!["%%literal".into()],
            }]
        );
    }

    #[test]
    fn reports_errors() {
        let mut parser = Parser::new();
        let events = feed(
            &mut parser,
            "%begin 1 7 1\ncan't find pane: %99\n%error 1 7 1\n",
        );
        assert_eq!(
            events,
            vec![Event::Reply {
                from_us: true,
                ok: false,
                lines: vec!["can't find pane: %99".into()]
            }]
        );
    }

    #[test]
    fn parses_notifications() {
        let mut parser = Parser::new();
        let events = feed(
            &mut parser,
            "%sessions-changed\n%layout-change @1 b25d,80x24,0,0,1 b25d,80x24,0,0,1 *\n%exit\n%exit server exited\n%client-detached /dev/pts/3\n%session-renamed $1 two words\n",
        );
        assert_eq!(
            events,
            vec![
                Event::Notification(Notification::SessionsChanged),
                Event::Notification(Notification::WindowsChanged),
                Event::Notification(Notification::Exit { reason: None }),
                Event::Notification(Notification::Exit {
                    reason: Some("server exited".into())
                }),
                Event::Notification(Notification::ClientsChanged),
                Event::Notification(Notification::SessionRenamed {
                    session: "$1".into(),
                    name: "two words".into(),
                }),
            ]
        );
    }

    #[test]
    fn encodes_input_as_hex() {
        assert_eq!(send_keys_hex(4, b"ls\r"), "send-keys -t %4 -H 6c 73 0d");
    }
}
