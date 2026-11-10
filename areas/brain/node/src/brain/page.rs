//! One corpus page as text: its front matter, title and slug, read the way
//! Context Citadel reads them (`/srv/context-citadel/lib/brain.js`), so the
//! reader and the search agree on what a page is called.

/// What a page says about itself.
#[derive(Debug, Default, PartialEq)]
pub struct Parsed<'a> {
    pub kind: String,
    pub summary: String,
    pub title: String,
    /// The text after the front matter.
    pub body: &'a str,
}

/// Reads a page at corpus-relative `path`. A page that does not open with a
/// `---` line has no front matter, and one whose front matter never closes is
/// all body.
pub fn parse<'a>(path: &str, text: &'a str) -> Parsed<'a> {
    let (front, body) = split_front_matter(text);
    let mut parsed = Parsed {
        body,
        ..Parsed::default()
    };
    for line in front.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "kind" => parsed.kind = scalar(value),
            "summary" => parsed.summary = scalar(value),
            _ => {}
        }
    }
    if parsed.kind.is_empty() && path.split('/').next() == Some("self") {
        parsed.kind = "self".to_owned();
    }
    parsed.title = title(body).unwrap_or_else(|| slug_of_path(path)).to_owned();
    parsed
}

fn split_front_matter(text: &str) -> (&str, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return ("", text);
    };
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return (&rest[..at], &rest[at + line.len()..]);
        }
        at += line.len();
    }
    ("", text)
}

/// A front matter value: JSON-quoted, single-quoted or plain.
fn scalar(raw: &str) -> String {
    let raw = raw.trim();
    if raw.starts_with('"')
        && let Ok(value) = serde_json::from_str::<String>(raw)
    {
        return value;
    }
    if let Some(inner) = raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        return inner.replace("''", "'");
    }
    raw.to_owned()
}

/// The first level-one heading.
fn title(body: &str) -> Option<&str> {
    body.lines()
        .filter_map(|line| line.strip_prefix('#'))
        .filter(|rest| rest.starts_with([' ', '\t']))
        .map(str::trim)
        .find(|heading| !heading.is_empty())
}

/// A page's slug: its file name without `.md`, or its folder's name for a
/// `README.md`.
pub fn slug_of_path(path: &str) -> &str {
    let mut parts = path.rsplit('/');
    let name = parts.next().unwrap_or(path);
    let stem = strip_md(name);
    match (stem, parts.next()) {
        ("README", Some(folder)) => folder,
        _ => stem,
    }
}

/// `name` without a trailing `.md`, in any case.
pub fn strip_md(name: &str) -> &str {
    match name.len().checked_sub(3) {
        Some(at) if name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(".md") => {
            &name[..at]
        }
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_front_matter_and_the_first_heading() {
        let text = "---\nkind: topic\nsummary: \"Tiles, \\\"seams\\\"\"\nfacets: [a, b]\n---\n\
                    Intro\n## Not this\n# Tiled upscaling\n";
        let page = parse("knowledge/upscaling/upscaling.md", text);
        assert_eq!(page.kind, "topic");
        assert_eq!(page.summary, "Tiles, \"seams\"");
        assert_eq!(page.title, "Tiled upscaling");
        assert_eq!(page.body, "Intro\n## Not this\n# Tiled upscaling\n");
    }

    #[test]
    fn a_page_without_front_matter_or_heading_is_named_by_its_slug() {
        let page = parse("self/telos/README.md", "just words\n");
        assert_eq!(page.title, "telos");
        assert_eq!(page.kind, "self");
        assert_eq!(page.body, "just words\n");
        let unclosed = parse("knowledge/x.md", "---\nkind: note\n# X\n");
        assert_eq!(unclosed.kind, "");
        assert_eq!(unclosed.title, "X");
    }

    #[test]
    fn single_quoted_values_unescape() {
        assert_eq!(scalar(" 'it''s' "), "it's");
        assert_eq!(scalar(" plain words "), "plain words");
    }

    #[test]
    fn slugs() {
        assert_eq!(slug_of_path("knowledge/a/notes.md"), "notes");
        assert_eq!(slug_of_path("knowledge/study/README.md"), "study");
        assert_eq!(slug_of_path("top.MD"), "top");
    }
}
