//! Keys that are not text: the bytes a terminal sends for them. Text itself
//! arrives through the platform input handler (IME, dead keys and AltGr
//! included).

use gpui::Keystroke;

pub fn bytes(k: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &k.modifiers;
    let csi = |s: &str| Some([b"\x1b".as_slice(), s.as_bytes()].concat());
    let mods = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control);
    let arrow = |c: char| {
        if mods > 1 {
            csi(&format!("[1;{mods}{c}"))
        } else if app_cursor {
            csi(&format!("O{c}"))
        } else {
            csi(&format!("[{c}"))
        }
    };
    let tilde = |n: u8| {
        if mods > 1 {
            csi(&format!("[{n};{mods}~"))
        } else {
            csi(&format!("[{n}~"))
        }
    };
    let f1_4 = |c: char| {
        if mods > 1 {
            csi(&format!("[1;{mods}{c}"))
        } else {
            csi(&format!("O{c}"))
        }
    };
    match k.key.as_str() {
        "enter" => Some(if m.alt {
            b"\x1b\r".to_vec()
        } else {
            b"\r".to_vec()
        }),
        "backspace" => Some(if m.control {
            b"\x08".to_vec()
        } else if m.alt {
            b"\x1b\x7f".to_vec()
        } else {
            b"\x7f".to_vec()
        }),
        "tab" => Some(if m.shift {
            b"\x1b[Z".to_vec()
        } else {
            b"\t".to_vec()
        }),
        "escape" => Some(b"\x1b".to_vec()),
        "up" => arrow('A'),
        "down" => arrow('B'),
        "right" => arrow('C'),
        "left" => arrow('D'),
        "home" => arrow('H'),
        "end" => arrow('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => f1_4('P'),
        "f2" => f1_4('Q'),
        "f3" => f1_4('R'),
        // Alt+F4 closes the window on Windows, as in every other program.
        "f4" if cfg!(windows) && mods == 3 => None,
        "f4" => f1_4('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if m.control => Some(vec![0]),
        key if m.control && !m.platform && key.chars().count() == 1 => {
            let c = key.as_bytes()[0];
            let ctl = match c {
                b'a'..=b'z' => c - b'a' + 1,
                b'@' | b'2' => 0,
                b'[' | b'3' => 0x1b,
                b'\\' | b'4' => 0x1c,
                b']' | b'5' => 0x1d,
                b'^' | b'6' => 0x1e,
                b'_' | b'-' | b'/' | b'7' => 0x1f,
                b'8' => 0x7f,
                _ => return None,
            };
            Some(if m.alt { vec![0x1b, ctl] } else { vec![ctl] })
        }
        key if m.alt && !m.control && key.chars().count() == 1 => {
            let ch = k.key_char.clone().unwrap_or_else(|| key.to_string());
            Some([b"\x1b".as_slice(), ch.as_bytes()].concat())
        }
        "space" if m.alt => Some(b"\x1b ".to_vec()),
        _ => None,
    }
}

/// Text for a paste: CRLF and LF become CR, as a terminal sends Enter, and
/// bracketed when the program asked for it.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // A paste must not end the bracket early.
        let text = text.replace("\x1b[201~", "");
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

/// Text for the clipboard: Windows programs expect CRLF between lines
/// (Notepad, Outlook and Windows Terminal copy so); elsewhere LF stays.
pub fn copied(text: String) -> String {
    if cfg!(windows) {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &'static str) -> Keystroke {
        Keystroke::parse(s).unwrap()
    }

    #[test]
    fn control_letters_and_arrows() {
        assert_eq!(bytes(&key("ctrl-c"), false), Some(vec![3]));
        assert_eq!(bytes(&key("ctrl-alt-a"), false), Some(vec![0x1b, 1]));
        assert_eq!(bytes(&key("up"), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(bytes(&key("up"), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(
            bytes(&key("ctrl-right"), false),
            Some(b"\x1b[1;5C".to_vec())
        );
        assert_eq!(bytes(&key("shift-tab"), false), Some(b"\x1b[Z".to_vec()));
        let alt_f4 = bytes(&key("alt-f4"), false);
        assert_eq!(alt_f4.is_none(), cfg!(windows), "Alt+F4 is Windows' close");
    }

    #[test]
    fn plain_text_is_left_to_the_input_handler() {
        assert_eq!(bytes(&key("a"), false), None);
        assert_eq!(bytes(&key("shift-a"), false), None);
    }

    #[test]
    fn a_paste_sends_enter_as_cr_and_brackets_when_asked() {
        assert_eq!(paste("a\r\nb\nc", false), b"a\rb\rc".to_vec());
        assert_eq!(paste("x", true), b"\x1b[200~x\x1b[201~".to_vec());
        assert_eq!(paste("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~".to_vec());
    }

    #[test]
    fn a_copy_ends_lines_as_the_platform_does() {
        let want = if cfg!(windows) {
            "a\r\nb\r\nc"
        } else {
            "a\nb\r\nc"
        };
        assert_eq!(copied("a\nb\r\nc".into()), want);
    }
}
