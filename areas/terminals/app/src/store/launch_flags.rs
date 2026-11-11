//! The catalogue edits the free-form flag line without losing quoted tokens.
pub use parvi_terminals_contract::Flag;

fn tokens(line: &str) -> Vec<(String, String)> {
    let (mut raw, mut text) = (String::new(), String::new());
    let mut quote = None;
    let mut result = Vec::new();
    for c in line.chars() {
        if let Some(q) = quote {
            raw.push(c);
            if c == q {
                quote = None;
            } else {
                text.push(c);
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            raw.push(c);
        } else if c.is_whitespace() {
            if !raw.is_empty() {
                result.push((std::mem::take(&mut raw), std::mem::take(&mut text)));
            }
        } else {
            raw.push(c);
            text.push(c);
        }
    }
    if !raw.is_empty() {
        result.push((raw, text));
    }
    result
}

fn names(token: &str, flag: &Flag) -> bool {
    token == flag.name
        || flag.short.as_deref() == Some(token)
        || token
            .strip_prefix(&flag.name)
            .is_some_and(|s| s.starts_with('='))
}

pub fn has(line: &str, flag: &Flag) -> bool {
    tokens(line).iter().any(|(_, t)| names(t, flag))
}

pub fn value(line: &str, flag: &Flag) -> String {
    let words = tokens(line);
    let Some(i) = words.iter().position(|(_, text)| names(text, flag)) else {
        return String::new();
    };
    if let Some(value) = words[i].1.strip_prefix(&format!("{}=", flag.name)) {
        return value.to_owned();
    }
    words
        .get(i + 1)
        .map(|(_, text)| text.clone())
        .unwrap_or_default()
}

pub fn remove(line: &str, flag: &Flag) -> String {
    let mut words = tokens(line);
    if let Some(i) = words.iter().position(|(_, t)| names(t, flag)) {
        let separate = flag.value.is_some() && !words[i].1.starts_with(&format!("{}=", flag.name));
        words.remove(i);
        if separate && i < words.len() {
            words.remove(i);
        }
    }
    words
        .into_iter()
        .map(|(raw, _)| raw)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn add(line: &str, flag: &Flag, value: &str) -> String {
    let mut words: Vec<String> = tokens(line).into_iter().map(|(raw, _)| raw).collect();
    words.push(flag.name.clone());
    if flag.value.is_some() && !value.is_empty() {
        words.push(if value.chars().any(char::is_whitespace) {
            format!("\"{value}\"")
        } else {
            value.to_owned()
        });
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_preserve_quoted_tokens_and_aliases() {
        let f = Flag {
            name: "--model".into(),
            short: Some("-m".into()),
            value: Some("<model>".into()),
            description: "Model".into(),
            values: vec![],
        };
        assert!(has("--other 'hello world' -m fast", &f));
        assert_eq!(
            remove("--other 'hello world' -m fast", &f),
            "--other 'hello world'"
        );
        assert_eq!(remove("--model=fast --verbose", &f), "--verbose");
        assert_eq!(value("--model=fast --verbose", &f), "fast");
        assert_eq!(value("-m 'very fast'", &f), "very fast");
        assert_eq!(
            add("--other 'hello world'", &f, "very fast"),
            "--other 'hello world' --model \"very fast\""
        );
    }
}
