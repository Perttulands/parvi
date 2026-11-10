//! Whether a pane's screen shows a program asking the operator something
//! (issue #18). Read from the screen the server already holds, after output,
//! never on a timer.
//!
//! Claude Code and Codex ask the same way: a numbered menu with one option
//! marked (`❯` in Claude Code, `›` in Codex), near the bottom of the screen,
//! with a footer that says Esc cancels, or options that answer yes or no. A
//! screen counts as waiting only when it has both, so other programs stay
//! active or idle, never wrongly waiting. Harness completion hooks (CHROTE
//! 2's `needs-input` event) can become a second source later.

pub mod harness;

/// The rows at the bottom of the screen that may hold the marked option.
/// Below it come the other options, the footer, and maybe a status line.
const MARK_WITHIN: usize = 10;

/// How far above the options the question may be.
const QUESTION_WITHIN: usize = 8;

/// The longest question kept, in characters.
const MAX_QUESTION: usize = 200;

/// The question a waiting screen asks, or `None` if it isn't waiting. The
/// question is empty if it isn't on screen, as when a narrow pane cuts it.
pub fn waiting<'a>(rows: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let rows: Vec<&str> = rows
        .into_iter()
        .map(unframe)
        .filter(|row| !row.is_empty())
        .collect();
    let bottom = rows.len().saturating_sub(MARK_WITHIN);
    let marked = (bottom..rows.len())
        .rev()
        .find(|&i| marked_option(rows[i]))?;
    let near = &rows[marked.saturating_sub(QUESTION_WITHIN)..];
    let answers = near
        .iter()
        .filter_map(|row| option_text(row))
        .any(|text| text.starts_with("Yes") || text.starts_with("No"));
    let footer = rows[marked..].iter().any(|row| {
        let row = row.to_lowercase();
        row.contains("esc to cancel") || row.contains("enter to confirm")
    });
    if !answers && !footer {
        return None;
    }
    // The options run up from the marked one; the question is above them.
    let mut first = marked;
    while first > 0 && option_text(rows[first - 1]).is_some() {
        first -= 1;
    }
    Some(question(
        &rows[first.saturating_sub(QUESTION_WITHIN)..first],
    ))
}

/// A row without a box's side borders, trimmed.
fn unframe(row: &str) -> &str {
    row.trim()
        .trim_start_matches(['│', '┃', '║'])
        .trim_end_matches(['│', '┃', '║'])
        .trim()
}

/// `❯ 1. Yes` or `› 2. No`: an option with the selection mark on it.
fn marked_option(row: &str) -> bool {
    row.strip_prefix(['❯', '›'])
        .is_some_and(|rest| option_text(rest.trim_start()).is_some())
}

/// The text of a numbered option, marked or not: `2. No, exit` gives
/// `No, exit`.
fn option_text(row: &str) -> Option<&str> {
    let row = row.strip_prefix(['❯', '›']).unwrap_or(row).trim_start();
    let digits = row.len() - row.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 || digits > 2 {
        return None;
    }
    row[digits..].strip_prefix(". ").map(str::trim_start)
}

/// The last sentence ending in `?` in these rows, joined across the rows a
/// narrow screen wrapped it over. Empty if there is none.
fn question(rows: &[&str]) -> String {
    let Some(end) = rows.iter().rposition(|row| row.contains('?')) else {
        return String::new();
    };
    let last = rows[end];
    let mut text = last[..=last.rfind('?').unwrap()].to_owned();
    // A wrapped question continues a row above that ends mid-sentence.
    let mut at = end;
    while at > 0 && !text.starts_with(char::is_uppercase) && !ends_sentence(rows[at - 1]) {
        at -= 1;
        text = format!("{} {text}", rows[at]);
    }
    // Only the sentence that asks.
    if let Some(start) = text[..text.len() - 1].rfind(['.', '!', '?', ':']) {
        let rest = text[start + 1..].trim_start();
        if rest.starts_with(char::is_uppercase) {
            text = rest.to_owned();
        }
    }
    match text.char_indices().nth(MAX_QUESTION) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

fn ends_sentence(row: &str) -> bool {
    row.ends_with(['.', '!', '?', ':'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Option<String> {
        let path = format!(
            "{}/tests/fixtures/waiting/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        waiting(text.lines())
    }

    #[test]
    fn claude_code_asks() {
        assert_eq!(
            fixture("claude-trust.txt").as_deref(),
            Some("Is this a project you created or one you trust?")
        );
        assert_eq!(
            fixture("claude-permission.txt").as_deref(),
            Some("Do you want to proceed?")
        );
    }

    #[test]
    fn codex_asks() {
        assert_eq!(
            fixture("codex-exec.txt").as_deref(),
            Some("Would you like to run the following command?")
        );
        assert_eq!(
            fixture("codex-network.txt").as_deref(),
            Some("Do you want to approve network access to \"example.com\"?")
        );
        // Cut off by a narrow pane: still waiting, with no question.
        assert_eq!(fixture("codex-narrow.txt").as_deref(), Some(""));
    }

    #[test]
    fn other_programs_never_wait() {
        for name in ["top.txt", "vim.txt", "shell.txt", "man-page.txt"] {
            assert_eq!(fixture(name), None, "{name}");
        }
    }

    #[test]
    fn a_menu_needs_answers_or_a_footer() {
        // A numbered list someone printed is not a question.
        assert_eq!(waiting(["Steps:", "› 1. Build", "  2. Test"]), None);
        // A marked menu with an Esc footer is, whatever its options say.
        assert_eq!(
            waiting(["Which model?", "❯ 1. Opus", "  2. Sonnet", "Esc to cancel"]).as_deref(),
            Some("Which model?")
        );
    }

    #[test]
    fn a_question_wrapped_over_rows_is_joined() {
        let rows = [
            "Would you like to run the following",
            "command?",
            "",
            "› 1. Yes, proceed (y)",
            "  2. No, and tell Codex what to do differently (esc)",
        ];
        assert_eq!(
            waiting(rows).as_deref(),
            Some("Would you like to run the following command?")
        );
    }

    #[test]
    fn a_boxed_prompt_is_read_inside_its_borders() {
        let rows = [
            "╭──────────────────────────╮",
            "│ Do you want to proceed?  │",
            "│ ❯ 1. Yes                 │",
            "│   2. No                  │",
            "╰──────────────────────────╯",
        ];
        assert_eq!(waiting(rows).as_deref(), Some("Do you want to proceed?"));
    }

    #[test]
    fn a_menu_that_scrolled_up_is_not_waiting() {
        let mut rows = vec!["Do you want to proceed?", "❯ 1. Yes", "  2. No"];
        rows.extend(["output"; MARK_WITHIN]);
        assert_eq!(waiting(rows), None);
    }
}
