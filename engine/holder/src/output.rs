//! Notifications the holder pushes to control clients.

/// `%output %<pane> <data>` with tmux's escaping: control bytes and the
/// backslash as `\ooo`.
pub fn line(pane: u32, data: &[u8]) -> Vec<u8> {
    let mut line = format!("%output %{pane} ").into_bytes();
    line.reserve(data.len() + data.len() / 8 + 1);
    for &b in data {
        if b < b' ' || b == b'\\' {
            line.extend_from_slice(&[
                b'\\',
                b'0' + (b >> 6),
                b'0' + ((b >> 3) & 7),
                b'0' + (b & 7),
            ]);
        } else {
            line.push(b);
        }
    }
    line.push(b'\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_controls_and_backslash() {
        assert_eq!(
            line(7, b"a\\b\r\n\x1b[m"),
            b"%output %7 a\\134b\\015\\012\\033[m\n"
        );
        assert_eq!(line(0, "ä".as_bytes()), "%output %0 ä\n".as_bytes());
    }
}
