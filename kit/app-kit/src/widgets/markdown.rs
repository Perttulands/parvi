//! Markdown drawn in the theme (CHROTE `Markdown.tsx`): headings,
//! paragraphs, lists with checkboxes, quotes, code, rules and tables, with
//! code, bold, emphasis and links inline. A link is handed to the caller,
//! which decides what it opens (a relative file, a work item, the web).
//! Files and Work both draw bodies with it.

use super::select::Selectable;
use crate::theme::{FAMILY, Theme, alpha, size};
use gpui::{
    AnyElement, App, Div, ElementId, FontStyle, FontWeight, HighlightStyle, InteractiveText,
    SharedString, StyledText, Window, div, prelude::*, px,
};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading(u8, String),
    Paragraph(String),
    /// Items with their depth, their marker (`None` bullet, `Some(n)`
    /// numbered) and their checkbox if any.
    List(Vec<Item>),
    Quote(String),
    Code(String),
    Rule,
    Table(Vec<Vec<String>>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub depth: usize,
    pub number: Option<u32>,
    pub checked: Option<bool>,
    pub text: String,
}

fn heading(line: &str) -> Option<(u8, String)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    let rest = &line[hashes..];
    ((1..=6).contains(&hashes) && (rest.is_empty() || rest.starts_with(' '))).then(|| {
        (
            hashes as u8,
            rest.trim().trim_end_matches('#').trim().to_string(),
        )
    })
}

fn list_item(line: &str) -> Option<Item> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    let (number, rest) = if let Some(r) = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
    {
        (None, r)
    } else {
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        let r = t[digits.len()..]
            .strip_prefix(". ")
            .or_else(|| t[digits.len()..].strip_prefix(") "))?;
        if digits.is_empty() {
            return None;
        }
        (digits.parse().ok(), r)
    };
    let (checked, text) = if let Some(r) = rest.strip_prefix("[ ] ") {
        (Some(false), r)
    } else if let Some(r) = rest
        .strip_prefix("[x] ")
        .or_else(|| rest.strip_prefix("[X] "))
    {
        (Some(true), r)
    } else {
        (None, rest)
    };
    Some(Item {
        depth: indent / 2,
        number,
        checked,
        text: text.trim().to_string(),
    })
}

fn rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

fn table_row(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    (t.starts_with('|') && t.ends_with('|') && t.len() > 1).then(|| {
        t[1..t.len() - 1]
            .split('|')
            .map(|c| c.trim().to_string())
            .collect()
    })
}

fn divider_row(cells: &[String]) -> bool {
    cells
        .iter()
        .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
}

/// `===` under a line makes it a first heading, `---` a second.
fn underline(t: &str) -> Option<u8> {
    let level = match t.chars().next()? {
        '=' => 1,
        '-' => 2,
        _ => return None,
    };
    (t.len() >= 2 && t.chars().all(|c| c == t.chars().next().unwrap_or(' '))).then_some(level)
}

pub fn parse(text: &str) -> Vec<Block> {
    let mut out = vec![];
    let mut para: Vec<&str> = vec![];
    let lines: Vec<&str> = text.lines().collect();
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Paragraph(para.join(" ")));
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if let Some(fence) = ["```", "~~~"].into_iter().find(|f| t.starts_with(f)) {
            flush(&mut para, &mut out);
            let mut code = vec![];
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with(fence) {
                code.push(lines[i]);
                i += 1;
            }
            out.push(Block::Code(code.join("\n")));
        } else if t.is_empty() {
            flush(&mut para, &mut out);
        } else if para.is_empty()
            && list_item(line).is_none()
            && let Some(level) = lines.get(i + 1).and_then(|n| underline(n.trim()))
        {
            // A setext heading: the line underlined with `===` or `---`.
            out.push(Block::Heading(level, t.to_string()));
            i += 1;
        } else if let Some((level, h)) = heading(t) {
            flush(&mut para, &mut out);
            out.push(Block::Heading(level, h));
        } else if rule(t) && para.is_empty() {
            out.push(Block::Rule);
        } else if let Some(q) = t.strip_prefix('>') {
            flush(&mut para, &mut out);
            let mut quote = vec![q.trim()];
            while i + 1 < lines.len() && lines[i + 1].trim().starts_with('>') {
                i += 1;
                quote.push(lines[i].trim()[1..].trim());
            }
            out.push(Block::Quote(quote.join(" ")));
        } else if let Some(item) = list_item(line) {
            flush(&mut para, &mut out);
            let mut items = vec![item];
            while i + 1 < lines.len() {
                let next = lines[i + 1];
                if let Some(it) = list_item(next) {
                    items.push(it);
                } else if !next.trim().is_empty()
                    && next.starts_with(' ')
                    && let Some(last) = items.last_mut()
                {
                    last.text.push(' ');
                    last.text.push_str(next.trim());
                } else {
                    break;
                }
                i += 1;
            }
            out.push(Block::List(items));
        } else if let Some(row) = table_row(t) {
            flush(&mut para, &mut out);
            let mut rows = vec![row];
            while i + 1 < lines.len()
                && let Some(r) = table_row(lines[i + 1])
            {
                if !divider_row(&r) {
                    rows.push(r);
                }
                i += 1;
            }
            out.push(Block::Table(rows));
        } else {
            para.push(t);
        }
        i += 1;
    }
    flush(&mut para, &mut out);
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    Code,
    Bold,
    Em,
    /// A link: the index into the links found.
    Link(usize),
}

/// Inline text without its marks, the styled ranges, and each link's target.
pub struct Runs {
    pub text: String,
    pub spans: Vec<(Range<usize>, Span)>,
    pub links: Vec<String>,
}

pub fn inline(src: &str, refs: bool) -> Runs {
    let mut text = String::new();
    let mut spans = vec![];
    let mut links = vec![];
    let b = src.as_bytes();
    let mut i = 0;
    while i < src.len() {
        let rest = &src[i..];
        if rest.starts_with('`')
            && let Some(end) = rest[1..].find('`')
        {
            let s = text.len();
            text.push_str(&rest[1..1 + end]);
            spans.push((s..text.len(), Span::Code));
            i += end + 2;
            continue;
        }
        if (rest.starts_with("**") || rest.starts_with("__"))
            && let Some(end) = rest[2..].find(&rest[..2])
            && end > 0
        {
            let s = text.len();
            text.push_str(&rest[2..2 + end]);
            spans.push((s..text.len(), Span::Bold));
            i += end + 4;
            continue;
        }
        if (rest.starts_with('*') || rest.starts_with('_'))
            && rest.len() > 2
            && !rest[1..].starts_with(' ')
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
            && let Some(end) = rest[1..].find(&rest[..1])
            && end > 0
        {
            let s = text.len();
            text.push_str(&rest[1..1 + end]);
            spans.push((s..text.len(), Span::Em));
            i += end + 2;
            continue;
        }
        if rest.starts_with('[')
            && let Some(close) = rest.find("](")
            && let Some(end) = rest[close..].find(')')
        {
            let s = text.len();
            text.push_str(&rest[1..close]);
            spans.push((s..text.len(), Span::Link(links.len())));
            links.push(rest[close + 2..close + end].to_string());
            i += close + end + 1;
            continue;
        }
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let end = rest
                .find(|c: char| c.is_whitespace() || c == ')' || c == '>')
                .unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',']);
            let s = text.len();
            text.push_str(url);
            spans.push((s..text.len(), Span::Link(links.len())));
            links.push(url.to_string());
            i += url.len();
            continue;
        }
        if refs && rest.starts_with('#') && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let digits: String = rest[1..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                let s = text.len();
                text.push('#');
                text.push_str(&digits);
                spans.push((s..text.len(), Span::Link(links.len())));
                links.push(format!("#{digits}"));
                i += digits.len() + 1;
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        text.push(c);
        i += c.len_utf8();
    }
    Runs { text, spans, links }
}

/// What a link click is given: the link's target as written.
pub type OnLink = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// How the text is drawn, whether `#123` counts as a link, and where its
/// texts are kept to be selected across (`select::track` on the area the
/// column scrolls in takes the mouse).
pub struct Look<'a> {
    pub t: &'a Theme,
    pub refs: bool,
    pub on_link: Option<OnLink>,
    pub select: Option<Rc<RefCell<Selectable>>>,
}

/// Separators between texts when copied: blocks a blank line apart, list
/// items and table rows a line apart, cells a tab apart.
const BLOCK: &str = "\n\n";
const LINE: &str = "\n";
const CELL: &str = "\t";

/// `text` drawn with `highlights`, selectable after `sep` when the look
/// keeps a selection.
fn text(
    look: &Look,
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    sep: &'static str,
) -> StyledText {
    match &look.select {
        Some(s) => s.borrow_mut().text(text, highlights, sep, look.t.selection),
        None => StyledText::new(SharedString::from(text)).with_highlights(highlights),
    }
}

fn styled(
    look: &Look,
    id: ElementId,
    src: &str,
    base: HighlightStyle,
    sep: &'static str,
) -> AnyElement {
    let t = look.t;
    let Runs { text, spans, links } = inline(src, look.refs);
    let mut highlights = vec![];
    let mut clickable = vec![];
    for (r, s) in spans {
        let style = match s {
            Span::Code => HighlightStyle {
                background_color: Some(alpha(t.text, 0.08)),
                color: Some(t.text),
                ..Default::default()
            },
            Span::Bold => HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            },
            Span::Em => HighlightStyle {
                font_style: Some(FontStyle::Italic),
                ..Default::default()
            },
            Span::Link(_) => {
                clickable.push(r.clone());
                HighlightStyle {
                    color: Some(t.accent),
                    underline: Some(gpui::UnderlineStyle {
                        color: Some(alpha(t.accent, 0.5)),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..Default::default()
                }
            }
        };
        highlights.push((r, style));
    }
    if base != HighlightStyle::default() && !text.is_empty() {
        highlights.insert(0, (0..text.len(), base));
    }
    let highlights = merge(highlights);
    let st = self::text(look, text, highlights, sep);
    match &look.on_link {
        Some(on) if !links.is_empty() => {
            let on = on.clone();
            let select = look.select.clone();
            InteractiveText::new(id, st)
                .on_click(clickable, move |i, window, cx| {
                    // A drag that selected words is not a click on a link.
                    if select
                        .as_ref()
                        .is_some_and(|s| !s.borrow().selection.is_empty())
                    {
                        return;
                    }
                    if let Some(l) = links.get(i) {
                        on(l, window, cx)
                    }
                })
                .into_any_element()
        }
        _ => st.into_any_element(),
    }
}

/// Highlights must not overlap: a base style is split around the others,
/// which take it on.
fn merge(mut h: Vec<(Range<usize>, HighlightStyle)>) -> Vec<(Range<usize>, HighlightStyle)> {
    if h.len() < 2 || h[0].0.start != 0 {
        return h;
    }
    let (all, base) = h.remove(0);
    if h.is_empty() {
        return vec![(all, base)];
    }
    let mut out = vec![];
    let mut at = 0;
    for (r, s) in h {
        if r.start > at {
            out.push((at..r.start, base));
        }
        out.push((r.clone(), base.highlight(s)));
        at = r.end;
    }
    if at < all.end {
        out.push((at..all.end, base));
    }
    out
}

/// The document as elements, one column.
pub fn render(look: &Look, id: &str, blocks: &[Block]) -> Div {
    let t = look.t;
    let mut col = div().flex().flex_col().gap(px(10.)).text_color(t.text);
    if let Some(s) = &look.select {
        s.borrow_mut().begin();
        let s = s.clone();
        col = col.on_children_prepainted(move |_, _, _| s.borrow_mut().laid_out());
    }
    for (n, b) in blocks.iter().enumerate() {
        let eid = ElementId::Name(format!("{id}-{n}").into());
        col = col.child(match b {
            Block::Heading(level, h) => {
                let sz = match level {
                    1 => 22.,
                    2 => 18.,
                    3 => 15.,
                    _ => size::TEXT,
                };
                div()
                    .pt(px(if *level <= 2 { 8. } else { 4. }))
                    .text_size(px(sz))
                    .font_weight(FontWeight::BOLD)
                    .when(*level <= 2, |d| {
                        d.pb(px(4.)).border_b_1().border_color(t.divider)
                    })
                    .child(styled(look, eid, h, HighlightStyle::default(), BLOCK))
            }
            Block::Paragraph(p) => div().line_height(px(20.)).child(styled(
                look,
                eid,
                p,
                HighlightStyle::default(),
                BLOCK,
            )),
            Block::Quote(q) => div()
                .pl(px(12.))
                .border_l_2()
                .border_color(t.divider)
                .text_color(t.text2)
                .child(styled(look, eid, q, HighlightStyle::default(), BLOCK)),
            Block::Code(c) => div()
                .p(px(10.))
                .bg(t.surface)
                .rounded(px(size::RADIUS))
                .font_family(FAMILY)
                .text_size(px(size::SMALL + 1.))
                .whitespace_nowrap()
                .child(text(look, c.replace('\t', "    "), vec![], BLOCK)),
            Block::Rule => div().h(px(1.)).bg(t.divider),
            Block::List(items) => {
                let mut list = div().flex().flex_col().gap(px(4.));
                for (k, it) in items.iter().enumerate() {
                    let marker = match (it.checked, it.number) {
                        (Some(true), _) => "☑".to_string(),
                        (Some(false), _) => "☐".to_string(),
                        (None, Some(n)) => format!("{n}."),
                        (None, None) => "•".to_string(),
                    };
                    let done = it.checked == Some(true);
                    let base = if done {
                        HighlightStyle {
                            color: Some(t.dim),
                            ..Default::default()
                        }
                    } else {
                        HighlightStyle::default()
                    };
                    list = list.child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .pl(px(it.depth as f32 * 18.))
                            .child(
                                div()
                                    .flex_none()
                                    .min_w(px(16.))
                                    .text_color(if it.checked.is_some() {
                                        t.accent
                                    } else {
                                        t.text2
                                    })
                                    .child(marker),
                            )
                            .child(div().flex_1().min_w_0().child(styled(
                                look,
                                ElementId::Name(format!("{id}-{n}-{k}").into()),
                                &it.text,
                                base,
                                if k == 0 { BLOCK } else { LINE },
                            ))),
                    );
                }
                list
            }
            Block::Table(rows) => {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
                let mut table = div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(t.divider)
                    .rounded(px(size::RADIUS));
                for (r, row) in rows.iter().enumerate() {
                    let mut line = div()
                        .flex()
                        .when(r > 0, |d| d.border_t_1().border_color(t.divider))
                        .when(r == 0, |d| {
                            d.bg(t.surface).font_weight(FontWeight::SEMIBOLD)
                        });
                    for c in 0..cols {
                        let cell = row.get(c).cloned().unwrap_or_default();
                        line = line.child(div().flex_1().min_w_0().px(px(8.)).py(px(4.)).child(
                            styled(
                                look,
                                ElementId::Name(format!("{id}-{n}-{r}-{c}").into()),
                                &cell,
                                HighlightStyle::default(),
                                match (r, c) {
                                    (0, 0) => BLOCK,
                                    (_, 0) => LINE,
                                    _ => CELL,
                                },
                            ),
                        ));
                    }
                    table = table.child(line);
                }
                table
            }
        });
    }
    col
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks() {
        let doc = "# Title\n\nSome *text*\nwraps.\n\n- [ ] todo\n- [x] done\n  more\n1. one\n\n```\ncode\n```\n> quote\n\n---\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let b = parse(doc);
        assert_eq!(b[0], Block::Heading(1, "Title".into()));
        assert_eq!(b[1], Block::Paragraph("Some *text* wraps.".into()));
        let Block::List(items) = &b[2] else {
            panic!("{:?}", b[2])
        };
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].checked, Some(false));
        assert_eq!(items[1].text, "done more");
        assert_eq!(items[2].number, Some(1));
        assert_eq!(b[3], Block::Code("code".into()));
        assert_eq!(b[4], Block::Quote("quote".into()));
        assert_eq!(b[5], Block::Rule);
        assert_eq!(
            b[6],
            Block::Table(vec![
                vec!["a".into(), "b".into()],
                vec!["1".into(), "2".into()]
            ])
        );
    }

    #[test]
    fn a_selection_across_the_blocks_copies_their_words_as_laid_out() {
        use crate::text::text_select::Spot;
        let doc = "# Title\n\nSome **bold** [link](x.md).\n\n- one\n- two\n\n```\nlet a = 1;\n\tb\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let t = Theme::violet();
        let select = Rc::new(RefCell::new(Selectable::default()));
        let look = Look {
            t: &t,
            refs: false,
            on_link: None,
            select: Some(select.clone()),
        };
        let _ = render(&look, "doc", &parse(doc));
        let mut s = select.borrow_mut();
        s.selection.press(Spot { text: 0, at: 2 });
        s.selection.drag(Spot {
            text: usize::MAX,
            at: 0,
        });
        assert_eq!(
            s.copied().as_deref(),
            Some("tle\n\nSome bold link.\n\none\ntwo\n\nlet a = 1;\n    b\n\na\tb\n1\t2")
        );
    }

    #[test]
    fn underlined_lines_are_headings() {
        let b = parse("Plan\n====\n\nSteps\n---\n- a\n");
        assert_eq!(b[0], Block::Heading(1, "Plan".into()));
        assert_eq!(b[1], Block::Heading(2, "Steps".into()));
        assert!(matches!(b[2], Block::List(_)));
    }

    #[test]
    fn inline_marks_and_links() {
        let Runs { text, spans, links } = inline(
            "Run `bd` on **this** and [docs](../a.md), see #12 or https://x.y/z.",
            true,
        );
        assert_eq!(text, "Run bd on this and docs, see #12 or https://x.y/z.");
        assert_eq!(spans[0], (4..6, Span::Code));
        assert_eq!(spans[1], (10..14, Span::Bold));
        assert_eq!(links, ["../a.md", "#12", "https://x.y/z"]);
        let none = inline("issue#12 and snake_case_name", true).links;
        assert!(none.is_empty());
    }

    #[test]
    fn a_base_style_is_split_around_marks() {
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let base = HighlightStyle {
            fade_out: Some(0.5),
            ..Default::default()
        };
        let m = merge(vec![(0..10, base), (2..4, bold)]);
        let ranges: Vec<_> = m.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(ranges, [0..2, 2..4, 4..10]);
    }
}
