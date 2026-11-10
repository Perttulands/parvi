//! What `send-keys` writes for tmux key names.

/// The bytes for one `send-keys` word: a key name, or the word itself.
pub fn bytes(word: &str) -> Vec<u8> {
    let named: &[u8] = match word {
        "Enter" => b"\r",
        "Escape" => b"\x1b",
        "Tab" => b"\t",
        "Space" => b" ",
        "BSpace" => b"\x7f",
        "Up" => b"\x1b[A",
        "Down" => b"\x1b[B",
        "Right" => b"\x1b[C",
        "Left" => b"\x1b[D",
        _ => {
            if let Some(&[letter]) = word.strip_prefix("C-").map(str::as_bytes)
                && letter.is_ascii_alphabetic()
            {
                return vec![letter.to_ascii_lowercase() - b'a' + 1];
            }
            return word.as_bytes().to_vec();
        }
    };
    named.to_vec()
}

/// Bytes from `send-keys -H` words.
pub fn hex(words: &[String]) -> Result<Vec<u8>, String> {
    words
        .iter()
        .map(|w| u8::from_str_radix(w, 16).map_err(|_| format!("bad hex: {w}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_controls() {
        assert_eq!(bytes("Enter"), b"\r");
        assert_eq!(bytes("C-c"), [3]);
        assert_eq!(bytes("C-Z"), [26]);
        assert_eq!(bytes("C-"), b"C-");
        assert_eq!(bytes("päivää"), "päivää".as_bytes());
        assert_eq!(hex(&["6c".into(), "1b".into()]).unwrap(), b"l\x1b");
        assert!(hex(&["zz".into()]).is_err());
    }
}
