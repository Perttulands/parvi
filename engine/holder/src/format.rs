//! tmux's `#{name}` formats, for the variables the holder knows.

/// Replaces every `#{name}` in `format` with `value(name)`. Unknown names
/// give an empty string, as in tmux.
pub fn expand(format: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(at) = rest.find("#{") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[at..]);
            return out;
        };
        out.push_str(&value(&after[..end]).unwrap_or_default());
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_known_and_blanks_unknown() {
        let value = |name: &str| (name == "pane_id").then(|| "%3".to_owned());
        assert_eq!(expand("#{pane_id} #{nope}x", value), "%3 x");
        assert_eq!(expand("plain #{unclosed", value), "plain #{unclosed");
    }
}
