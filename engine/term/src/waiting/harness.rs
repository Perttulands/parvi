//! Recognition of native harness screens. Recognition does not send keys.
use crate::Screen;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
    Pi,
}

fn lines(screen: &Screen) -> Vec<String> {
    screen
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .filter(|cell| cell.width != 0)
                .map(|cell| {
                    if cell.text.is_empty() {
                        " "
                    } else {
                        cell.text.as_str()
                    }
                })
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// The active prompt is near the bottom; older prompt glyphs belong to history.
fn prompt_row(harness: Harness, rows: &[String]) -> Option<usize> {
    let glyph = match harness {
        Harness::Claude => '❯',
        Harness::Codex => '›',
        Harness::Pi => return None,
    };
    let start = rows
        .iter()
        .rposition(|line| !line.trim().is_empty())?
        .saturating_sub(7);
    (start..rows.len())
        .rev()
        .find(|&i| rows[i].trim_start().starts_with(glyph))
}

/// The active status is the nearest nonempty row above the editor, ignoring
/// the horizontal rule Claude draws around it.
fn status_above(rows: &[String], editor: usize) -> Option<&str> {
    rows[editor.saturating_sub(5)..editor]
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty() && !line.chars().all(|c| c == '─' || c == '━'))
}

/// Whether the harness is currently running a turn.
pub fn working(harness: Harness, screen: &Screen) -> bool {
    let rows = lines(screen);
    if harness == Harness::Pi {
        return pi_editor(&rows).unwrap_or(false);
    }
    let status = prompt_row(harness, &rows)
        .and_then(|row| status_above(&rows, row))
        .or_else(|| {
            rows.iter()
                .rev()
                .find(|row| !row.trim().is_empty())
                .map(String::as_str)
        });
    status.is_some_and(|line| working_line(harness, line))
}
fn working_line(harness: Harness, line: &str) -> bool {
    let line = line.trim_start();
    match harness {
        Harness::Codex => {
            line.starts_with('•')
                && line
                    .split_once(" (")
                    .is_some_and(|(_, detail)| detail.contains(" • esc to interrupt)"))
        }
        Harness::Claude => {
            let spinner = line.starts_with(['✶', '✢', '✻', '✽', '✳', '✱', '✦', '✧', '⎿']);
            spinner
                && (line.contains("esc to interrupt")
                    || line.split('…').skip(1).any(|tail| {
                        tail.trim_start().strip_prefix('(').is_some_and(|s| {
                            let n = s.bytes().take_while(u8::is_ascii_digit).count();
                            n > 0 && matches!(s.as_bytes().get(n), Some(b's' | b'm' | b'h'))
                        })
                    }))
        }
        Harness::Pi => {
            (line.starts_with('⠋') || line.starts_with('•'))
                && [
                    "esc to cancel",
                    "esc to interrupt",
                    "escape to cancel",
                    "escape to interrupt",
                ]
                .iter()
                .any(|s| line.contains(s))
        }
    }
}
fn plain_rule(line: &str) -> bool {
    let line = line.trim();
    let suffix = line.trim_start_matches('─');
    if suffix.len() == line.len() {
        return false;
    }
    if suffix.is_empty() {
        return true;
    }
    let Some(rest) = suffix
        .strip_prefix(" ↑ ")
        .or_else(|| suffix.strip_prefix(" ↓ "))
    else {
        return false;
    };
    let count = rest.bytes().take_while(u8::is_ascii_digit).count();
    count > 0
        && rest[count..]
            .strip_prefix(" more ")
            .is_some_and(|s| s.chars().all(|c| c == '─'))
}
fn pi_editor(lines: &[String]) -> Option<bool> {
    let bottom = lines.iter().rposition(|line| plain_rule(line))?;
    let top = (0..bottom).rev().find(|&i| {
        let s = lines[i].trim();
        s.starts_with('─') && s.ends_with('─')
    })?;
    Some(
        !plain_rule(&lines[top])
            || status_above(lines, top).is_some_and(|s| working_line(Harness::Pi, s)),
    )
}

/// Text in the active editor, with wrapping and placeholder whitespace
/// removed. `None` means the cursor is not known to be in an editor.
/// Callers must also check `ready` before pasting or submitting text.
pub fn input(harness: Harness, screen: &Screen) -> Option<String> {
    let rows = lines(screen);
    if harness == Harness::Pi {
        let bottom = rows.iter().rposition(|line| plain_rule(line))?;
        let top = (0..bottom).rev().find(|&i| {
            let line = rows[i].trim();
            line.starts_with('─') && line.ends_with('─')
        })?;
        return Some(rendered(&rows[top + 1..bottom].join("")));
    }
    if !screen.cursor.visible {
        return None;
    }
    let cursor = usize::from(screen.cursor.row);
    if cursor >= screen.rows.len() {
        return None;
    }
    let glyph = match harness {
        Harness::Claude => "❯",
        Harness::Codex => "›",
        Harness::Pi => unreachable!(),
    };
    for top in (0..=cursor).rev() {
        let row = &screen.rows[top];
        if row.first().is_some_and(|cell| cell.text == glyph) {
            let mut text = String::new();
            for (index, cells) in screen.rows[top..=cursor].iter().enumerate() {
                for cell in cells.iter().skip(usize::from(index == 0)) {
                    if !cell.dim && cell.width != 0 {
                        text.push_str(&cell.text);
                    }
                }
            }
            return Some(rendered(&text));
        }
        let line = rows[top].trim();
        if line.is_empty() || line.chars().all(|c| c == '─' || c == '━') {
            return None;
        }
    }
    None
}

/// Normalize a pasted pointer and its wrapped rendering the same way.
pub fn rendered(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && !('\u{2800}'..='\u{28ff}').contains(c))
        .collect()
}

/// Whether a pointer can be pasted without overwriting an operator's input.
pub fn input_clear(harness: Harness, screen: &Screen) -> bool {
    if !ready(harness, screen) || input(harness, screen).as_deref() != Some("") {
        return false;
    }
    harness == Harness::Pi || screen.cursor.column == 2
}
/// Whether a ready input is visible, even when it contains operator text.
pub fn ready(harness: Harness, screen: &Screen) -> bool {
    let rows = lines(screen);
    if harness == Harness::Pi {
        return pi_editor(&rows).is_some_and(|busy| !busy);
    }
    let Some(prompt) = prompt_row(harness, &rows) else {
        return false;
    };
    if status_above(&rows, prompt).is_some_and(|line| working_line(harness, line)) {
        return false;
    }
    // The prompt and its immediate surroundings, not old conversation text,
    // determine whether startup or a trust menu is still active.
    let start = prompt.saturating_sub(3);
    let end = (prompt + 4).min(rows.len());
    let area = rows[start..end].join("\n");
    match harness {
        Harness::Codex => ![
            "loading",
            "Do you trust",
            "Yes, continue",
            "Trust this folder?",
            "1. Trust and continue",
        ]
        .iter()
        .any(|s| area.contains(s)),
        Harness::Claude => !["Quick safety check", "Yes, I trust this folder"]
            .iter()
            .any(|s| area.contains(s)),
        Harness::Pi => unreachable!(),
    }
}
/// Recognizes a first-use dialog. The caller must also ensure that no brief
/// was sent and that the cursor is outside the input editor before answering.
pub fn trust_dialog(harness: Harness, screen: &Screen) -> bool {
    let rows = lines(screen);
    // A quoted dialog in scrollback must never cause a key in an editor.
    let cursor = usize::from(screen.cursor.row);
    if screen.cursor.visible
        && rows.iter().enumerate().any(|(i, line)| {
            let line = line.trim_start();
            let prompt = line.strip_prefix('❯').or_else(|| line.strip_prefix('›'));
            i <= cursor
                && prompt.is_some_and(|rest| {
                    let rest = rest.trim_start();
                    !rest.starts_with(|c: char| c.is_ascii_digit())
                        && !rest.starts_with("Yes, I trust")
                        && !rest.starts_with("No, exit")
                })
        })
    {
        return false;
    }
    let text = rows.join("\n");
    match harness {
        Harness::Claude => text.contains("Yes, I trust this folder"),
        Harness::Codex => {
            text.contains("Do you trust the contents of this directory?")
                && text.contains("1. Yes, continue")
                || text.contains("Trust this folder?") && text.contains("1. Trust and continue")
        }
        Harness::Pi => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screen(text: &str) -> Screen {
        let mut p = vt100::Parser::new(24, 120, 0);
        p.process(text.replace('\n', "\r\n").as_bytes());
        Screen::from_vt(p.screen())
    }
    #[test]
    fn input_reads_only_the_active_editor_and_ignores_dim_placeholders() {
        let empty = screen("› \x1b[2mAsk anything\x1b[22m\x1b[1;3H");
        assert_eq!(input(Harness::Codex, &empty).as_deref(), Some(""));
        assert!(input_clear(Harness::Codex, &empty));
        let misplaced = screen("› \x1b[2mAsk anything\x1b[22m\x1b[1;4H");
        assert!(!input_clear(Harness::Codex, &misplaced));
        let typed = screen("› /tmp/a brief\r\n  continued");
        assert_eq!(
            input(Harness::Codex, &typed).as_deref(),
            Some("/tmp/abriefcontinued")
        );
        assert_eq!(
            input(Harness::Codex, &screen("› old\n\nTrust this folder?")),
            None
        );
        let pi = screen("answer\n────────\n operator text\n────────\n/work");
        assert_eq!(input(Harness::Pi, &pi).as_deref(), Some("operatortext"));
    }
    #[test]
    fn ready_and_working_follow_native_panes() {
        assert!(ready(Harness::Claude, &screen("Claude Code\n❯ ")));
        assert!(ready(Harness::Codex, &screen("OpenAI Codex\n› ")));
        assert!(!ready(
            Harness::Claude,
            &screen("Claude Code\n✶ Puzzling… (4s · tokens)\n❯ ")
        ));
        assert!(working(
            Harness::Claude,
            &screen("✶ Puzzling… (4s · tokens)")
        ));
        assert!(!working(Harness::Codex, &screen("Summarised… (3s read)")));
        assert!(ready(
            Harness::Pi,
            &screen("answer\n────────\n\n────────\n/work")
        ));
        let busy = screen("answer\n── Working ──\n\n────────\n/work");
        assert!(!ready(Harness::Pi, &busy));
        assert!(working(Harness::Pi, &busy));
    }
    #[test]
    fn trust_dialog_is_not_quoted_text_above_operator_input() {
        let dialog = "Trust this folder?\n› 1. Trust and continue\n  2. Cancel";
        assert!(trust_dialog(Harness::Codex, &screen(dialog)));
        assert!(!trust_dialog(
            Harness::Codex,
            &screen(&format!("{dialog}\n\n› typing here"))
        ));
    }
    #[test]
    fn reused_seats_are_ready_after_the_banner_scrolls_out() {
        for (harness, prompt) in [(Harness::Claude, "❯ draft"), (Harness::Codex, "› draft")] {
            let content = format!(
                "{}\n{prompt}\nmodel status",
                (0..30)
                    .map(|i| format!("output {i}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            let screen = screen(&content);
            assert!(ready(harness, &screen), "{harness:?}");
            assert!(!working(harness, &screen), "{harness:?}");
        }
    }
    #[test]
    fn stale_or_quoted_working_text_does_not_hide_the_idle_prompt() {
        for (harness, prompt, stale) in [
            (Harness::Claude, "❯", "✶ Puzzling… (4s · tokens)"),
            (Harness::Codex, "›", "• Working (8s • esc to interrupt)"),
        ] {
            let s = screen(&format!(
                "{stale}\n• The manual quotes esc to interrupt.\n{prompt}\nmodel status"
            ));
            assert!(!working(harness, &s), "{harness:?}");
            assert!(ready(harness, &s), "{harness:?}");
            let active = screen(&format!("old answer\n{stale}\n{prompt}\nmodel status"));
            assert!(working(harness, &active), "{harness:?}");
            assert!(!ready(harness, &active), "{harness:?}");
            let distant = screen(&format!("{stale}\n\n\n\n\n\n{prompt}\nmodel status"));
            assert!(!working(harness, &distant), "{harness:?}");
            assert!(ready(harness, &distant), "{harness:?}");
        }
        let pi = screen("The manual quotes escape to cancel.\n────────\n\n────────\n/work");
        assert!(!working(Harness::Pi, &pi));
        assert!(ready(Harness::Pi, &pi));
    }
}
