//! The links a page makes and the pages they name.
//!
//! A page links by `[[slug]]` (with an optional `#anchor` and `|label`) and by
//! relative Markdown links to `.md` files, never from inside code: these are
//! Context Citadel's rules (`linksOf` in `lib/brain.js`). A wiki target is
//! resolved by slug and then by title; a file link by its path in the corpus
//! and then by its slug, so a link still finds a page that has moved folders.

use std::collections::HashMap;

use super::page::{slug_of_path, strip_md};

/// One link as written in a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The target as the page writes it: the wiki target, or the file link's
    /// href without its anchor. The reader matches links in the text by it.
    pub written: String,
    pub to: Target,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A wiki link's target.
    Wiki(String),
    /// A file link, as a corpus-relative path.
    File(String),
}

/// Every link `body`, the page at corpus-relative `path`, makes, in order.
pub fn references(path: &str, body: &str) -> Vec<Reference> {
    let text = without_code(body);
    let mut found = Vec::new();
    for line in text.lines() {
        wiki_links(line, &mut found);
        file_links(path, line, &mut found);
    }
    found
}

/// The body with fenced blocks and inline code removed: code shows syntax, it
/// does not link.
fn without_code(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut fence: Option<&str> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(open) = fence {
            if trimmed.starts_with(open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = ["```", "~~~"].into_iter().find(|f| trimmed.starts_with(f)) {
            fence = Some(open);
            continue;
        }
        // An unmatched backtick is text, as in Citadel's `[^`\n]*` match.
        let mut rest = line;
        while let Some(open) = rest.find('`') {
            let Some(close) = rest[open + 1..].find('`') else {
                break;
            };
            out.push_str(&rest[..open]);
            rest = &rest[open + close + 2..];
        }
        out.push_str(rest);
        out.push('\n');
    }
    out
}

fn wiki_links(line: &str, found: &mut Vec<Reference>) {
    let mut rest = line;
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("]]") else {
            break;
        };
        let inner = &after[..close];
        if inner.contains(['[', ']']) {
            rest = &rest[open + 1..];
            continue;
        }
        let target = inner.split(['|', '#']).next().unwrap_or("").trim();
        if !target.is_empty() && !inner.starts_with(['|', '#']) {
            found.push(Reference {
                written: target.to_owned(),
                to: Target::Wiki(target.to_owned()),
            });
        }
        rest = &after[close + 2..];
    }
}

fn file_links(path: &str, line: &str, found: &mut Vec<Reference>) {
    let mut rest = line;
    while let Some(open) = rest.find("](") {
        rest = &rest[open + 2..];
        let inner = rest.strip_prefix('<').unwrap_or(rest);
        let run = inner
            .find(|c: char| c == ')' || c == '>' || c.is_whitespace())
            .unwrap_or(inner.len());
        let tail = &inner[run..];
        if !(tail.starts_with(')') || tail.starts_with(">)")) {
            continue;
        }
        let Some(href) = href_of(&inner[..run]) else {
            continue;
        };
        if let Some(to) = corpus_path(path, href) {
            found.push(Reference {
                written: href.to_owned(),
                to: Target::File(to),
            });
        }
    }
}

/// The `.md` file a link run names: the longest prefix ending in `.md` that
/// is the whole run or is followed by an anchor.
fn href_of(run: &str) -> Option<&str> {
    (3..=run.len())
        .rev()
        .filter(|&end| run.is_char_boundary(end))
        .find(|&end| {
            run[..end].ends_with(".md") && (end == run.len() || run[end..].starts_with('#'))
        })
        .map(|end| &run[..end])
}

/// The corpus-relative path a relative href from `from` names, or `None` for
/// absolute paths and URLs.
fn corpus_path(from: &str, href: &str) -> Option<String> {
    let scheme = href
        .split_once("://")
        .is_some_and(|(s, _)| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphabetic()));
    if scheme || href.starts_with(['/', '~']) {
        return None;
    }
    let decoded = percent_decoded(href);
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

/// Every page by path, slug and title, to resolve links against.
#[derive(Default)]
pub struct Index {
    by_path: HashMap<String, usize>,
    by_slug: HashMap<String, usize>,
    by_title: HashMap<String, usize>,
}

impl Index {
    /// Indexes pages given as (path, title), in order; where two share a slug
    /// or title, the first one listed wins.
    pub fn new<'a>(pages: impl IntoIterator<Item = (&'a str, &'a str)>) -> Index {
        let mut index = Index::default();
        for (i, (path, title)) in pages.into_iter().enumerate() {
            index.by_path.entry(path.to_owned()).or_insert(i);
            index
                .by_slug
                .entry(slug_of_path(path).to_owned())
                .or_insert(i);
            index.by_title.entry(title.to_lowercase()).or_insert(i);
        }
        index
    }

    /// The position of the page `target` names.
    pub fn resolve(&self, target: &Target) -> Option<usize> {
        match target {
            Target::Wiki(written) => {
                let slug = strip_md(written.rsplit('/').next().unwrap_or(written));
                self.by_slug
                    .get(slug)
                    .or_else(|| self.by_title.get(&written.to_lowercase()))
            }
            Target::File(path) => self
                .by_path
                .get(path)
                .or_else(|| self.by_slug.get(slug_of_path(path))),
        }
        .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(path: &str, body: &str) -> Vec<String> {
        references(path, body)
            .into_iter()
            .map(|r| r.written)
            .collect()
    }

    #[test]
    fn finds_wiki_and_file_links_outside_code() {
        let body = "See [[tiling]] and [[Seam blending|seams]] or [[tiling#overlap]].\n\
                    ```toml\n[[steps]]\n```\n\
                    Inline `[[not-a-link]]` and [bench](../bench/results.md#table).\n\
                    [site](https://example.com/x.md) [abs](/etc/x.md) [img](pic.png)\n";
        assert_eq!(
            written("knowledge/upscaling/upscaling.md", body),
            ["tiling", "Seam blending", "tiling", "../bench/results.md"]
        );
        let refs = references("knowledge/upscaling/upscaling.md", body);
        assert_eq!(
            refs[3].to,
            Target::File("knowledge/bench/results.md".into())
        );
    }

    #[test]
    fn file_links_decode_and_take_angle_brackets() {
        let refs = references("a/b.md", "[x](<my%20notes.md>) [y](./c/d.md)");
        let to: Vec<_> = refs.into_iter().map(|r| r.to).collect();
        assert_eq!(
            to,
            [
                Target::File("a/my notes.md".into()),
                Target::File("a/c/d.md".into())
            ]
        );
    }

    #[test]
    fn an_unmatched_backtick_is_text() {
        assert_eq!(written("a.md", "it`s [[here]]"), ["here"]);
    }

    #[test]
    fn resolves_by_slug_title_and_path() {
        let index = Index::new([
            ("knowledge/tiling/tiling.md", "Tiling"),
            ("knowledge/seams.md", "Seam blending"),
            ("evidence/study/README.md", "A study"),
        ]);
        let wiki = |t: &str| index.resolve(&Target::Wiki(t.into()));
        let file = |p: &str| index.resolve(&Target::File(p.into()));
        assert_eq!(wiki("tiling"), Some(0));
        assert_eq!(wiki("seam blending"), Some(1));
        assert_eq!(wiki("study"), Some(2));
        assert_eq!(wiki("missing"), None);
        assert_eq!(file("knowledge/seams.md"), Some(1));
        assert_eq!(file("old/place/seams.md"), Some(1));
        assert_eq!(file("old/nothing.md"), None);
    }
}
