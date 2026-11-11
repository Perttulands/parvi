//! An item's text as the card reads it: every Bead id the board knows,
//! written in the prose, becomes a link that selects that Bead (CHROTE's
//! `BeadText`). Code, links and addresses stay as written.

/// The scheme of a link to a Bead by its id.
pub const BEAD_LINK: &str = "bead-id:";

/// `body` with each word `is_id` accepts written as a Markdown link
/// `[id](bead-id:id)`.
pub fn link_ids(body: &str, is_id: &dyn Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(body.len());
    let mut fenced = false;
    for line in body.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        if fenced {
            out.push_str(line);
        } else {
            link_line(line, is_id, &mut out);
        }
    }
    out
}

fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')
}

fn link_line(line: &str, is_id: &dyn Fn(&str) -> bool, out: &mut String) {
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        // Kept whole: a code span, a link, an address.
        let kept = if let Some(code) = rest.strip_prefix('`') {
            code.find('`').map(|e| e + 2)
        } else if rest.starts_with('[') {
            rest.find("](")
                .and_then(|c| rest[c..].find(')').map(|e| c + e + 1))
        } else if rest.starts_with("http://") || rest.starts_with("https://") {
            Some(rest.find(char::is_whitespace).unwrap_or(rest.len()))
        } else {
            None
        };
        if let Some(n) = kept {
            out.push_str(&rest[..n]);
            i += n;
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        let after_word = line[..i]
            .chars()
            .next_back()
            .is_some_and(|p| word_char(p) || p == '/' || p == '#');
        if word_char(c) && !after_word {
            let len = rest.find(|c: char| !word_char(c)).unwrap_or(rest.len());
            let word = rest[..len].trim_end_matches(['.', '-', '_']);
            if word.contains('-') && is_id(word) {
                out.push_str(&format!("[{word}]({BEAD_LINK}{word})"));
                i += word.len();
                continue;
            }
            out.push_str(&rest[..len]);
            i += len;
            continue;
        }
        out.push(c);
        i += c.len_utf8();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_the_ids_the_board_knows_and_leaves_code_links_and_addresses() {
        let known = |w: &str| matches!(w, "ctx-xvl" | "library-zvi.1");
        let text = "Waits on ctx-xvl. See library-zvi.1, not ctx-gone; `ctx-xvl` in code, \
                    [ctx-xvl](https://x/ctx-xvl) and https://x/ctx-xvl stay.\n```\nctx-xvl\n```\n";
        assert_eq!(
            link_ids(text, &known),
            "Waits on [ctx-xvl](bead-id:ctx-xvl). See [library-zvi.1](bead-id:library-zvi.1), \
             not ctx-gone; `ctx-xvl` in code, [ctx-xvl](https://x/ctx-xvl) and \
             https://x/ctx-xvl stay.\n```\nctx-xvl\n```\n"
        );
    }
}
