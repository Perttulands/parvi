//! Adapt wiki links to the shared Markdown renderer, keeping code literal.

/// Turns `[[target#heading|label]]` into a Markdown link, outside code.
pub fn wiki_links(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut fence: Option<&str> = None;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(open) = fence {
            if trimmed.starts_with(open) {
                fence = None;
            }
            out.push_str(line);
            continue;
        }
        if let Some(open) = ["```", "~~~"]
            .into_iter()
            .find(|open| trimmed.starts_with(open))
        {
            fence = Some(open);
            out.push_str(line);
            continue;
        }
        let mut rest = line;
        while !rest.is_empty() {
            if rest.starts_with('`')
                && let Some(end) = rest[1..].find('`')
            {
                let end = end + 2;
                out.push_str(&rest[..end]);
                rest = &rest[end..];
                continue;
            }
            if rest.starts_with("[[")
                && let Some(end) = rest.find("]]")
            {
                let inner = &rest[2..end];
                let (target, label) = inner.split_once('|').unwrap_or((inner, inner));
                if !target.trim().is_empty() && !inner.contains(['[', ']']) {
                    out.push('[');
                    out.push_str(label.trim());
                    out.push_str("](");
                    out.push_str(target.trim());
                    out.push(')');
                    rest = &rest[end + 2..];
                    continue;
                }
            }
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// GitHub-style heading anchors, numbered when headings repeat.
#[derive(Default)]
pub struct Slugger {
    seen: std::collections::HashMap<String, usize>,
}
impl Slugger {
    pub fn slug(&mut self, heading: &str) -> String {
        let base = slug(heading);
        let mut result = base.clone();
        while self.seen.contains_key(&result) {
            let n = self.seen.entry(base.clone()).or_default();
            *n += 1;
            result = format!("{base}-{n}");
        }
        self.seen.insert(result.clone(), 0);
        result
    }
}
pub fn slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// Remove the first heading when it repeats the reader's title.
pub fn without_title(body: &str, title: &str) -> String {
    let lines: Vec<&str> = body.split('\n').collect();
    if let Some(at) = lines.iter().position(|line| !line.trim().is_empty())
        && let Some(heading) = lines[at].strip_prefix("# ")
        && heading.trim() == title
    {
        return lines[at + 1..].join("\n");
    }
    body.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headings_are_slugged_as_github_does_and_repeats_numbered() {
        let mut s = Slugger::default();
        assert_eq!(s.slug("More details"), "more-details");
        assert_eq!(s.slug("The method: v2!"), "the-method-v2");
        assert_eq!(s.slug("Päivä ja yö"), "päivä-ja-yö");
        assert_eq!(s.slug("More details"), "more-details-1");
        assert_eq!(s.slug("More details"), "more-details-2");
        assert_eq!(s.slug("more-details-1"), "more-details-1-1");
    }
    #[test]
    fn drops_the_heading_that_repeats_the_title() {
        assert_eq!(without_title("\n# Pace\nbody", "Pace"), "body");
        assert_eq!(without_title("# Other\nbody", "Pace"), "# Other\nbody");
    }
    #[test]
    fn reads_the_blocks_a_page_uses() {
        use parvi_app_kit::widgets::markdown::{Block, inline, parse};
        let blocks = parse(
            "# Title\n\nSome *em* and **strong** with `code`.\n\n- one\n- two\n\n> quoted\n\n```\nfn x() {}\n```\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        );
        assert!(matches!(&blocks[0], Block::Heading(1, _)));
        let Block::Paragraph(text) = &blocks[1] else {
            panic!("{blocks:?}")
        };
        assert_eq!(inline(text, false).text, "Some em and strong with code.");
        assert!(matches!(&blocks[2], Block::List(items) if items.len() == 2));
        assert!(matches!(&blocks[3], Block::Quote(_)));
        assert_eq!(blocks[4], Block::Code("fn x() {}".into()));
        assert_eq!(blocks[5], Block::Rule);
        assert!(matches!(&blocks[6], Block::Table(rows) if rows.len() == 2 && rows[1][1] == "2"));
    }
    #[test]
    fn reads_wiki_links_with_anchor_and_label() {
        let source = wiki_links("See [[self/pace#Notes|the pace]] and [[decision-models]].");
        let runs = parvi_app_kit::widgets::markdown::inline(&source, false);
        assert_eq!(runs.text, "See the pace and decision-models.");
        assert_eq!(runs.links, ["self/pace#Notes", "decision-models"]);
    }
    #[test]
    fn a_markdown_link_keeps_its_target_and_anchor() {
        let runs = parvi_app_kit::widgets::markdown::inline("[Pace](self/pace.md#top)", false);
        assert_eq!(runs.links, ["self/pace.md#top"]);
        assert_eq!(runs.links[0].split('#').next(), Some("self/pace.md"));
    }
    #[test]
    fn wiki_links_are_rendered_without_touching_code() {
        assert_eq!(
            wiki_links("[[first#part|read]] `[[literal]]`\n```\n[[code]]\n```\n"),
            "[read](first#part) `[[literal]]`\n```\n[[code]]\n```\n"
        );
    }
}
