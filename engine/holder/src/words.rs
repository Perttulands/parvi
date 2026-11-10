//! tmux-style command lines: words, quotes and flags.

/// Splits a command line into words. `'...'` is literal; `"..."` takes
/// backslash escapes (`\n`, `\t`, `\r`, `\\`, `\"`, `\$`, `\ooo`).
pub fn split(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => word.push('\n'),
                            Some('r') => word.push('\r'),
                            Some('t') => word.push('\t'),
                            Some(d @ '0'..='7') => {
                                let mut v = d as u32 - '0' as u32;
                                for _ in 0..2 {
                                    if let Some(&d @ '0'..='7') = chars.peek() {
                                        v = v * 8 + (d as u32 - '0' as u32);
                                        chars.next();
                                    }
                                }
                                word.push(char::from_u32(v).unwrap_or('?'));
                            }
                            Some(other) => word.push(other),
                            None => {}
                        },
                        c => word.push(c),
                    }
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// Quotes a word so [`split`] gives it back.
pub fn quote(word: &str) -> String {
    let mut q = String::from("\"");
    for c in word.chars() {
        match c {
            '"' | '\\' | '$' => {
                q.push('\\');
                q.push(c);
            }
            '\n' => q.push_str("\\n"),
            '\r' => q.push_str("\\r"),
            '\t' => q.push_str("\\t"),
            c if (c as u32) < 0x20 => q.push_str(&format!("\\{:03o}", c as u32)),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

/// A command's flags and arguments, tmux-style: `-abc`, `-t value` or
/// `-tvalue` for the letters in `valued`, `--` ends flags, and the first
/// argument ends them too.
pub struct Args {
    pub flags: Vec<(char, Option<String>)>,
    pub rest: Vec<String>,
}

impl Args {
    pub fn parse(words: &[String], valued: &str) -> Args {
        let mut flags = Vec::new();
        let mut rest = Vec::new();
        let mut i = 0;
        while i < words.len() {
            let w = &words[i];
            if w == "--" {
                rest.extend_from_slice(&words[i + 1..]);
                break;
            }
            if w.len() > 1 && w.starts_with('-') {
                for (at, c) in w[1..].char_indices() {
                    if valued.contains(c) {
                        let tail = &w[1 + at + c.len_utf8()..];
                        let value = if !tail.is_empty() {
                            tail.to_owned()
                        } else {
                            i += 1;
                            words.get(i).cloned().unwrap_or_default()
                        };
                        flags.push((c, Some(value)));
                        break;
                    }
                    flags.push((c, None));
                }
                i += 1;
                continue;
            }
            rest.extend_from_slice(&words[i..]);
            break;
        }
        Args { flags, rest }
    }

    pub fn has(&self, c: char) -> bool {
        self.flags.iter().any(|(f, _)| *f == c)
    }

    pub fn get(&self, c: char) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(f, _)| *f == c)
            .and_then(|(_, v)| v.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_tmux() {
        assert_eq!(
            split(r#"display-message -p -t %1 '#{cursor_x} #{cursor_y}'"#),
            [
                "display-message",
                "-p",
                "-t",
                "%1",
                "#{cursor_x} #{cursor_y}"
            ]
        );
        assert_eq!(
            split(r#"set-buffer -b x -- "a \"q\" \$ \001""#)[4],
            "a \"q\" $ \u{1}"
        );
        let text = "päivää\n\"x\"\t$y";
        assert_eq!(split(&format!("x {}", quote(text)))[1], text);
    }

    #[test]
    fn parses_flags() {
        let a = Args::parse(&split("-p -e -J -S - -t %3"), "tSEFb");
        assert!(a.has('p') && a.has('J'));
        assert_eq!(a.get('S'), Some("-"));
        assert_eq!(a.get('t'), Some("%3"));
        let a = Args::parse(&split("-t %3 -H 6c 73"), "t");
        assert!(a.has('H'));
        assert_eq!(a.rest, ["6c", "73"]);
    }
}
