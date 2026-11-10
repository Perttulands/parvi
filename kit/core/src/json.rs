//! JSON text written by hand where a frame's bytes are fixed.

/// Appends `value` as a JSON string.
pub fn push_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_quotes_backslashes_and_controls() {
        let mut out = String::new();
        super::push_string(&mut out, "say \"hi\"\\\n");
        assert_eq!(out, r#""say \"hi\"\\\u000a""#);
    }
}
