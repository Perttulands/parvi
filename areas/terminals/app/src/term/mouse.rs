//! Mouse reports for programs that asked for them (SGR or the X10 form).

use alacritty_terminal::term::TermMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button, or the release of any (X10).
    None,
}

/// What the program asked to hear about.
pub fn wants(mode: TermMode, motion: bool, held: bool) -> bool {
    if !motion {
        return mode.intersects(TermMode::MOUSE_MODE);
    }
    mode.contains(TermMode::MOUSE_MOTION) || (held && mode.contains(TermMode::MOUSE_DRAG))
}

/// One report at a zero-based cell. `pressed` is false for a release;
/// `motion` adds the motion flag.
pub fn report(
    mode: TermMode,
    button: Button,
    pressed: bool,
    motion: bool,
    col: usize,
    row: usize,
    mods: (bool, bool, bool),
) -> Vec<u8> {
    let mut code: u32 = match button {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::None => 3,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
    };
    let (shift, alt, ctrl) = mods;
    code += 4 * u32::from(shift) + 8 * u32::from(alt) + 16 * u32::from(ctrl);
    if motion {
        code += 32;
    }
    let (x, y) = (col + 1, row + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if pressed { 'M' } else { 'm' };
        return format!("\x1b[<{code};{x};{y}{end}").into_bytes();
    }
    // The X10 form has no release per button: a release is button 3.
    if !pressed {
        code = (code & !3) | 3;
    }
    let enc = |v: usize| (v.min(223) as u8) + 32;
    vec![
        0x1b,
        b'[',
        b'M',
        (code as u8).wrapping_add(32),
        enc(x),
        enc(y),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_reports_press_release_and_wheel() {
        let m = TermMode::SGR_MOUSE | TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            report(m, Button::Left, true, false, 0, 0, (false, false, false)),
            b"\x1b[<0;1;1M"
        );
        assert_eq!(
            report(m, Button::Left, false, false, 4, 2, (false, false, false)),
            b"\x1b[<0;5;3m"
        );
        assert_eq!(
            report(m, Button::WheelUp, true, false, 9, 9, (false, false, true)),
            b"\x1b[<80;10;10M"
        );
    }

    #[test]
    fn x10_reports_release_as_button_three() {
        let m = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            report(m, Button::Right, false, false, 0, 0, (false, false, false)),
            vec![0x1b, b'[', b'M', 35, 33, 33]
        );
    }
}
