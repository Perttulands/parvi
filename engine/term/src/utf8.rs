//! Normalizes broken UTF-8 in pane output before the emulator and browsers
//! see it. Older tmux drops a broken character; 3.6 and later display a
//! replacement character. Escape sequences run before the replacement, as
//! they do in tmux, and a character split across output chunks is held.

/// A pane's character in progress, across output chunks.
#[derive(Debug, Default)]
pub struct Utf8Gate {
    held: [u8; 4],
    len: usize,
    /// The bytes the held character needs in all; 0 when none is held.
    need: usize,
    replace_invalid: bool,
    escape: Escape,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    Start,
    Csi,
    Osc,
    String,
    DcsEntry,
    Dcs,
    DcsEsc,
    Intermediate,
}

/// tmux 3.6 changed broken UTF-8 from being discarded to being replaced.
pub fn tmux_replaces_invalid(version: &str) -> bool {
    let version = version.trim();
    let mut parts = version.strip_prefix("next-").unwrap_or(version).split('.');
    let major = parts.next().and_then(|s| s.parse::<u32>().ok());
    let minor = parts.next().and_then(|s| {
        s.trim_end_matches(|c: char| !c.is_ascii_digit())
            .parse::<u32>()
            .ok()
    });
    matches!((major, minor), (Some(m), Some(n)) if (m, n) >= (3, 6))
}

impl Utf8Gate {
    pub fn new(replace_invalid: bool) -> Self {
        Self {
            replace_invalid,
            ..Self::default()
        }
    }
    /// Hold incomplete characters and normalize broken ones. Valid output is
    /// returned as is.
    pub fn pass(&mut self, data: Vec<u8>) -> Vec<u8> {
        if self.need == 0 && std::str::from_utf8(&data).is_ok() {
            return data;
        }
        let mut out = Vec::with_capacity(data.len() + self.len);
        for byte in data {
            if self.escape != Escape::None {
                // tmux treats CAN and SUB as C0 controls in escape states:
                // they cancel the sequence and end any incomplete character.
                // DCS payloads are the exception: there they are data.
                if matches!(byte, 0x18 | 0x1a)
                    && !matches!(self.escape, Escape::Dcs | Escape::DcsEsc)
                {
                    out.push(byte);
                    out.extend_from_slice("�".as_bytes());
                    self.reset();
                    continue;
                }
                out.push(byte);
                self.escape = match (self.escape, byte) {
                    (Escape::Start, b'[') => Escape::Csi,
                    (Escape::Start, b']') => Escape::Osc,
                    (Escape::Start, b'P') => Escape::DcsEntry,
                    (Escape::Start, b'X' | b'^' | b'_') => Escape::String,
                    (Escape::Start, 0x20..=0x2f) => Escape::Intermediate,
                    (Escape::Csi, 0x40..=0x7e) | (Escape::Intermediate, 0x30..=0x7e) => {
                        Escape::None
                    }
                    (Escape::DcsEntry, 0x40..=0x7e) => Escape::Dcs,
                    (Escape::Dcs, 0x1b) => Escape::DcsEsc,
                    (Escape::DcsEsc, b'\\') | (Escape::Osc, 0x07) => Escape::None,
                    (Escape::DcsEsc, _) => Escape::Dcs,
                    (_, 0x1b) => Escape::Start,
                    (Escape::Start, _) => Escape::None,
                    (state, _) => state,
                };
                continue;
            }
            if self.need > 0 {
                if self.replace_invalid && byte == 0x1b {
                    out.push(byte);
                    self.escape = Escape::Start;
                    continue;
                }
                if byte >= 0x80 {
                    self.held[self.len] = byte;
                    self.len += 1;
                    if self.len == self.need {
                        if std::str::from_utf8(&self.held[..self.len]).is_err() {
                            if self.replace_invalid {
                                out.extend_from_slice("�".as_bytes());
                            }
                        } else {
                            out.extend_from_slice(&self.held[..self.len]);
                        }
                        self.reset();
                    }
                    continue;
                }
                // ASCII breaks the held character and is handled normally.
                if self.replace_invalid {
                    out.extend_from_slice("�".as_bytes());
                }
                self.reset();
            }
            let need = match byte {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                // ASCII and controls pass through. tmux either drops an
                // invalid high byte or renders a replacement for it.
                _ => {
                    if byte < 0x80 {
                        out.push(byte);
                    } else if self.replace_invalid {
                        out.extend_from_slice("�".as_bytes());
                    }
                    continue;
                }
            };
            self.held[0] = byte;
            self.len = 1;
            self.need = need;
        }
        out
    }

    /// Forgets a character in progress, as when the screen is reseeded.
    pub fn reset(&mut self) {
        self.len = 0;
        self.need = 0;
        self.escape = Escape::None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(chunks: &[&[u8]]) -> Vec<u8> {
        let mut gate = Utf8Gate::default();
        chunks.iter().flat_map(|c| gate.pass(c.to_vec())).collect()
    }

    #[test]
    fn tmux_versions_choose_broken_character_behavior() {
        assert!(!tmux_replaces_invalid("3.5a"));
        assert!(tmux_replaces_invalid("3.6a"));
        assert!(tmux_replaces_invalid("3.8"));
        assert!(tmux_replaces_invalid("next-3.8"));
        assert!(!tmux_replaces_invalid(""));
    }

    #[test]
    fn modern_tmux_replaces_a_broken_character_after_an_escape() {
        for broken in [&b"\xc3"[..], &b"\xe3\xc3"[..]] {
            let mut gate = Utf8Gate::new(true);
            let mut screen = vt100::Parser::new(5, 20, 0);
            screen.render_replacement();
            let mut start = b"garbage ".to_vec();
            start.extend_from_slice(broken);
            start.extend_from_slice(b"\x1b[");
            for chunk in [&start[..], &b"H\x1b[Jclean"[..]] {
                screen.process(&gate.pass(chunk.to_vec()));
            }
            assert_eq!(screen.screen().contents(), "�clean");
        }
    }

    #[test]
    fn cancellation_and_restart_work_across_chunks() {
        for cancel in [0x18, 0x1a] {
            let mut gate = Utf8Gate::new(true);
            assert_eq!(gate.pass(b"\xc3\x1b[31".to_vec()), b"\x1b[31");
            assert_eq!(
                gate.pass(vec![cancel, b'X']),
                [vec![cancel], "�X".as_bytes().to_vec()].concat()
            );
        }
        let mut gate = Utf8Gate::new(true);
        assert_eq!(
            gate.pass(b"\xc3\x1b[31\x1b]0;title".to_vec()),
            b"\x1b[31\x1b]0;title"
        );
        assert_eq!(gate.pass(b"\x07X".to_vec()), "\x07�X".as_bytes());

        let mut gate = Utf8Gate::new(true);
        assert_eq!(gate.pass(b"\xc3\x1bPq\x18".to_vec()), b"\x1bPq\x18");
        assert_eq!(gate.pass(b"\x1b\\X".to_vec()), "\x1b\\�X".as_bytes());
    }

    #[test]
    fn valid_output_passes_untouched() {
        assert_eq!(
            pass(&[b"plain \xc3\xa9 \xe3\x81\x82 \xf0\x9f\x98\x80"]),
            b"plain \xc3\xa9 \xe3\x81\x82 \xf0\x9f\x98\x80"
        );
    }

    #[test]
    fn a_character_split_across_chunks_is_held_until_complete() {
        let mut gate = Utf8Gate::default();
        assert_eq!(gate.pass(b"a\xe3\x81".to_vec()), b"a");
        assert_eq!(gate.pass(b"\x82b".to_vec()), b"\xe3\x81\x82b");
    }

    #[test]
    fn a_broken_character_is_dropped_and_what_broke_it_kept() {
        assert_eq!(pass(&[b"garbage \xe3", b"\x1b[H"]), b"garbage \x1b[H");
        assert_eq!(pass(&[b"x\xc3\x07y"]), b"x\x07y");
        assert_eq!(pass(&[b"m\xe3\x81n"]), b"mn");
        // tmux consumes all high bytes needed by the original character.
        assert_eq!(pass(&[b"\xe3\xc3\xa9"]), b"");
    }

    #[test]
    fn old_tmux_discards_stray_bytes() {
        assert_eq!(pass(&[b"a\x80b\xffc"]), b"abc");
    }

    /// The issue's reproduction, through the gate into the emulator. Lead
    /// bytes `\xe3`, `\xc3`, `\xf0\x9f` and `\xe3\x81` all used to swallow
    /// the escape; tmux shows each as here.
    #[test]
    fn the_emulator_then_matches_tmux() {
        for (cut, then, shows, cursor) in [
            (
                &b"garbage \xe3"[..],
                &b"\x1b[H\x1b[2Jclean"[..],
                "clean",
                (0, 5),
            ),
            (b"ab\xe3", b"\x1b[1mcd", "abcd", (0, 4)),
            (b"x\xc3", b"\x07y", "xy", (0, 2)),
            (b"p\xf0\x9f", b"\rq", "q", (0, 1)),
            (b"m\xe3\x81", b"n", "mn", (0, 2)),
        ] {
            let mut gate = Utf8Gate::default();
            let mut screen = vt100::Parser::new(5, 20, 0);
            screen.process(&gate.pass(cut.to_vec()));
            screen.process(&gate.pass(then.to_vec()));
            assert_eq!(screen.screen().contents(), shows, "{then:?}");
            assert_eq!(screen.screen().cursor_position(), cursor, "{then:?}");
        }
    }
}
