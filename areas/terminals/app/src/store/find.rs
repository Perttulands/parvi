//! A scrollback search hit as the drawer shows it.

/// `line` cut around the first hit of `query`, by the node's rule (smart
/// case: a capital letter matches case and all): at most `lead` characters
/// before the hit, `…` where cut, then the hit, then the rest. `None` when
/// the hit is not in the line.
pub fn hit_parts(line: &str, query: &str, lead: usize) -> Option<(String, String, String)> {
    if query.is_empty() {
        return None;
    }
    let exact = query.chars().any(char::is_uppercase);
    let chars: Vec<char> = line.chars().collect();
    let needle: Vec<char> = query.chars().collect();
    let same = |a: &char, b: &char| {
        if exact {
            a == b
        } else {
            a.to_lowercase().eq(b.to_lowercase())
        }
    };
    let at = (0..=chars.len().checked_sub(needle.len())?).find(|&i| {
        chars[i..i + needle.len()]
            .iter()
            .zip(&needle)
            .all(|(a, b)| same(a, b))
    })?;
    let from = at.saturating_sub(lead);
    let mut before: String = chars[from..at].iter().collect();
    if from > 0 {
        before.insert(0, '…');
    }
    let hit = chars[at..at + needle.len()].iter().collect();
    let after = chars[at + needle.len()..].iter().collect();
    Some((before, hit, after))
}

/// The hit Enter (`forward`) or Shift+Enter opens among `n`: the lit one
/// (`cursor`) when it is not the one `shown`, else the next or the one
/// before, round the ends.
pub fn step(n: usize, cursor: usize, shown: Option<usize>, forward: bool) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let cursor = cursor.min(n - 1);
    Some(match (shown == Some(cursor), forward) {
        (false, _) => cursor,
        (true, true) => (cursor + 1) % n,
        (true, false) => (cursor + n - 1) % n,
    })
}

#[cfg(test)]
mod tests {
    use super::{hit_parts, step};

    #[test]
    fn enter_opens_the_lit_hit_then_steps_round() {
        assert_eq!(step(0, 0, None, true), None);
        assert_eq!(
            step(3, 0, None, true),
            Some(0),
            "the first Enter opens the lit one"
        );
        assert_eq!(step(3, 0, Some(0), true), Some(1));
        assert_eq!(step(3, 2, Some(2), true), Some(0), "round the end");
        assert_eq!(step(3, 0, Some(0), false), Some(2), "and back round");
        assert_eq!(
            step(3, 1, Some(0), true),
            Some(1),
            "a row picked by arrows opens first"
        );
    }

    #[test]
    fn the_hit_stays_in_view_with_a_little_before_it() {
        let line = format!("{}srv-djgu closed", "x".repeat(200));
        let (before, hit, after) = hit_parts(&line, "srv-djgu", 12).unwrap();
        assert_eq!(before, format!("…{}", "x".repeat(12)));
        assert_eq!(hit, "srv-djgu");
        assert_eq!(after, " closed");
        // Smart case, as the node matches.
        assert_eq!(
            hit_parts("An ERROR here", "error", 40),
            Some(("An ".into(), "ERROR".into(), " here".into()))
        );
        assert_eq!(hit_parts("error", "Error", 40), None);
        assert_eq!(hit_parts("short", "", 40), None);
    }
}
