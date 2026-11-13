//! Links in a pane's text, found by shape alone, as CHROTE's terminal finds
//! them (`terminal/pathLinks.ts` and xterm's web-links addon): a web address
//! opens in the browser, an absolute path opens in Files on the pane's
//! device. Nothing is asked of a node to find one.
//!
//! The text is read the way it reads on screen: rows the terminal wrapped
//! are one line, a wide character is one character in its two cells. A
//! path that a program broke at the right margin itself (Claude Code does)
//! is joined only when it is enclosed in quotes or brackets and every row
//! it continues on has the same indentation; anything less is a guess.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Url(String),
    Path(String),
    WorkItem(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub target: Target,
    /// The cells the link is drawn on, in grid lines.
    pub cells: Vec<Point>,
}

/// How many rows either side of the pointer are read: enough for any path
/// an agent prints, bounded for a screen of unbroken output.
const RADIUS: i32 = 32;

/// The link drawn on cell `p`, if any.
pub fn at<T>(term: &Term<T>, p: Point) -> Option<Link> {
    let grid = term.grid();
    if p.line < grid.topmost_line() || p.line > grid.bottommost_line() {
        return None;
    }
    let mut p = p;
    if p.column.0 > 0 && grid[p].flags.contains(Flags::WIDE_CHAR_SPACER) {
        p.column -= 1;
    }
    let text = Text::read(term, p.line - RADIUS, p.line + RADIUS);
    text.links().into_iter().find(|l| l.cells.contains(&p))
}

/// A path as the session's device reads it: one relative to the session's
/// folder `cwd` is joined to it.
pub fn resolve(cwd: Option<&str>, path: &str) -> String {
    match cwd {
        Some(cwd) if !path.starts_with('/') => format!(
            "{}/{}",
            cwd.trim_end_matches('/'),
            path.trim_start_matches("./")
        ),
        _ => path.to_string(),
    }
}

/// The links with a cell on grid lines `first..=last`.
pub fn in_lines<T>(term: &Term<T>, first: Line, last: Line) -> Vec<Link> {
    // A path an agent broke over a few rows may begin above the first.
    const ABOVE: i32 = 4;
    let mut links: Vec<Link> = Text::read(term, first - ABOVE, last)
        .links()
        .into_iter()
        .filter(|l| l.cells.iter().any(|c| c.line >= first && c.line <= last))
        .collect();
    links.sort_by_key(|l| l.cells.first().copied());
    links
}

/// Rows read as one text, each character with the cell it is drawn in.
struct Text {
    chars: Vec<char>,
    /// The cell of each character; `None` for the break between two rows.
    cells: Vec<Option<Point>>,
    /// The leading spaces of each row read, from `first`.
    indents: Vec<usize>,
    first: Line,
    cols: usize,
    /// The text begins or ends partway through a wrapped line.
    cut_before: bool,
    cut_after: bool,
}

/// The row continues on the next one: the terminal wrapped it.
fn wrapped<T>(term: &Term<T>, line: Line) -> bool {
    let grid = term.grid();
    line >= grid.topmost_line()
        && line <= grid.bottommost_line()
        && grid[line][Column(term.columns() - 1)]
            .flags
            .contains(Flags::WRAPLINE)
}

impl Text {
    fn read<T>(term: &Term<T>, first: Line, last: Line) -> Self {
        let grid = term.grid();
        let cols = term.columns();
        let first = first.max(grid.topmost_line());
        let last = last.min(grid.bottommost_line());
        let mut t = Text {
            chars: Vec::new(),
            cells: Vec::new(),
            indents: Vec::new(),
            first,
            cols,
            cut_before: wrapped(term, first - 1),
            cut_after: wrapped(term, last),
        };
        let mut line = first;
        while line <= last {
            let row = &grid[line];
            let start = t.chars.len();
            for col in 0..cols {
                let cell = &row[Column(col)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let at = Point::new(line, Column(col));
                for c in
                    std::iter::once(cell.c).chain(cell.zerowidth().into_iter().flatten().copied())
                {
                    t.chars.push(c);
                    t.cells.push(Some(at));
                }
            }
            t.indents
                .push(t.chars[start..].iter().take_while(|c| **c == ' ').count());
            // Spaces before a wrap are part of the line; a row's own end is not.
            if !wrapped(term, line) {
                while t.chars.len() > start && t.chars.last() == Some(&' ') {
                    t.chars.pop();
                    t.cells.pop();
                }
                if line < last {
                    t.chars.push('\n');
                    t.cells.push(None);
                }
            }
            line += 1;
        }
        t
    }

    fn cells(&self, from: usize, len: usize) -> Vec<Point> {
        self.cells[from..from + len]
            .iter()
            .flatten()
            .copied()
            .collect()
    }

    fn links(&self) -> Vec<Link> {
        let mut links: Vec<Link> = urls(&self.chars)
            .into_iter()
            .map(|(i, n)| Link {
                target: Target::Url(self.chars[i..i + n].iter().collect()),
                cells: self.cells(i, n),
            })
            .collect();
        let joined = self.enclosed();
        let end = self.chars.len();
        for (i, n) in paths(&self.chars) {
            let overlaps = joined.iter().any(|(s, e, _)| i < *e && i + n > *s);
            // A path cut off by the rows read is not offered in part.
            let cut = (i == 0 && self.cut_before) || (i + n == end && self.cut_after);
            if !overlaps && !cut {
                links.push(Link {
                    target: Target::Path(self.chars[i..i + n].iter().collect()),
                    cells: self.cells(i, n),
                });
            }
        }
        links.extend(joined.into_iter().map(|(_, _, l)| l));
        // A path relative to the pane's folder, where nothing above is.
        for (i, n) in relative_paths(&self.chars) {
            let cut = (i == 0 && self.cut_before) || (i + n == end && self.cut_after);
            let cells = self.cells(i, n);
            if !cut
                && !links
                    .iter()
                    .any(|link| cells.iter().any(|cell| link.cells.contains(cell)))
            {
                links.push(Link {
                    target: Target::Path(self.chars[i..i + n].iter().collect()),
                    cells,
                });
            }
        }
        // A URL or path takes precedence over a work-like substring.
        for (i, n) in work_items(&self.chars) {
            let cut = (i == 0 && self.cut_before) || (i + n == end && self.cut_after);
            let cells = self.cells(i, n);
            if !cut
                && !links
                    .iter()
                    .any(|link| cells.iter().any(|cell| link.cells.contains(cell)))
            {
                links.push(Link {
                    target: Target::WorkItem(self.chars[i..i + n].iter().collect()),
                    cells,
                });
            }
        }
        links
    }

    /// Paths a program broke at the margin itself, inside quotes or
    /// brackets: `("/a/b` then, on the next row at the same indentation,
    /// `c/d")`. Each with the span of its text.
    fn enclosed(&self) -> Vec<(usize, usize, Link)> {
        let t = &self.chars;
        let mut found = Vec::new();
        let mut i = 0;
        while i + 1 < t.len() {
            let Some(close) = closing(t[i]) else {
                i += 1;
                continue;
            };
            if t[i + 1] != '/' {
                i += 1;
                continue;
            }
            let start = i + 1;
            let mut j = run(t, start);
            let mut breaks = Vec::new();
            while j < t.len() && t[j] == '\n' {
                let spaces = t[j + 1..].iter().take_while(|c| **c == ' ').count();
                let after = run(t, j + 1 + spaces);
                if spaces == 0 || after == j + 1 + spaces {
                    break;
                }
                breaks.push((j, spaces));
                j = after;
            }
            if breaks.is_empty() || j >= t.len() || closing_char(t[j]).is_none() {
                i += 1;
                continue;
            }
            if t[j] == close
                && let Some(link) = self.joined(start, j, &breaks)
            {
                found.push((start, j, link));
            }
            i = j + 1;
        }
        found
    }

    /// The path in `start..end` across its `breaks`, when every break is at
    /// the right margin and indented as the row the path starts on.
    fn joined(&self, start: usize, end: usize, breaks: &[(usize, usize)]) -> Option<Link> {
        let row = |i: usize| self.cells[i].map(|p| (p.line.0 - self.first.0) as usize);
        let indent = self.indents[row(start)?];
        if indent == 0 {
            return None;
        }
        for &(b, spaces) in breaks {
            let before = self.cells[b - 1]?;
            if spaces != indent || before.column.0 != self.cols - 1 {
                return None;
            }
        }
        let kept: Vec<usize> = (start..end)
            .filter(|&i| !self.chars[i].is_whitespace())
            .collect();
        let text: Vec<char> = kept.iter().map(|&i| self.chars[i]).collect();
        let (s, n) = paths(&text).into_iter().next()?;
        let whole = trailing_trimmed(&text);
        if s != 0 || n != whole {
            return None;
        }
        Some(Link {
            target: Target::Path(text[..n].iter().collect()),
            cells: kept[..n].iter().filter_map(|&i| self.cells[i]).collect(),
        })
    }
}

fn closing(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '"' => Some('"'),
        '\'' => Some('\''),
        _ => None,
    }
}

fn closing_char(c: char) -> Option<char> {
    matches!(c, ')' | ']' | '"' | '\'').then_some(c)
}

/// A character of a file name.
fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_.@%+~-".contains(c)
}

/// Where a run of path characters (names and slashes) from `i` ends.
fn run(t: &[char], i: usize) -> usize {
    let mut j = i;
    while j < t.len() && (name_char(t[j]) || t[j] == '/') {
        j += 1;
    }
    j
}

const TRAILING: &str = ".,;:)]}'\"/";

/// The length of `t` without the punctuation that ends a sentence.
fn trailing_trimmed(t: &[char]) -> usize {
    let mut n = t.len();
    while n > 0 && TRAILING.contains(t[n - 1]) {
        n -= 1;
    }
    n
}

/// The absolute paths in `t`, as (start, length): a slash that begins a
/// word, then names and slashes, less the punctuation after it. The slash
/// inside a web address follows a letter or a slash, so it is not one.
fn paths(t: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < t.len() {
        let follows_word =
            i > 0 && (t[i - 1].is_ascii_alphanumeric() || "_./~:@%+-".contains(t[i - 1]));
        if t[i] != '/' || follows_word {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        loop {
            let s = j;
            while j < t.len() && name_char(t[j]) {
                j += 1;
            }
            if j > s && j < t.len() && t[j] == '/' {
                j += 1;
            } else {
                break;
            }
        }
        let n = trailing_trimmed(&t[i..j]);
        // A slash on its own is not somewhere to go.
        if n > 1 {
            out.push((i, n));
        }
        i = j;
    }
    out
}

/// The paths relative to the pane's folder in `t`, as (start, length):
/// a word that names a file by its extension (`notes.txt`,
/// `src/ui/rail.rs:12`) or starts at `./` or `../`. A word with slashes but
/// no extension (`and/or`) or a number (`v2.1.3`) is not one, nor a repo
/// before its `#42`.
fn relative_paths(t: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < t.len() {
        let starts = (name_char(t[i]) || t[i] == '.')
            && (i == 0 || !(name_char(t[i - 1]) || "/:#".contains(t[i - 1])));
        if !starts {
            i += 1;
            continue;
        }
        let j = run(t, i);
        let n = trailing_trimmed(&t[i..j]);
        let word: String = t[i..i + n].iter().collect();
        if t.get(j) != Some(&'#') && names_a_file(&word) {
            out.push((i, n));
        }
        i = j.max(i + 1);
    }
    out
}

/// A relative word that reads as a file: `./` or `../` before it, or a
/// last name with an extension of letters and digits after a name with a
/// letter in it.
fn names_a_file(word: &str) -> bool {
    if word.starts_with('/') || word.contains("//") {
        return false;
    }
    if (word.starts_with("./") || word.starts_with("../")) && word.len() > 3 {
        return true;
    }
    let last = word.rsplit('/').next().unwrap_or(word);
    let Some((base, ext)) = last.rsplit_once('.') else {
        return false;
    };
    base.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 1
        && base.trim_start_matches('.').chars().count() >= 2
        && (1..=8).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && ext.chars().any(|c| c.is_ascii_alphabetic())
}

/// Recognize complete tokens, so a path or a larger word is never offered as
/// a partial item id. Soft-wrapped rows have already been joined by `Text`.
fn work_items(t: &[char]) -> Vec<(usize, usize)> {
    use parvi_work_contract::work_ref::{is_bead_id, is_repo_part};
    let token = |c: char| c.is_ascii_alphanumeric() || "_./-#".contains(c);
    let mut found = Vec::new();
    let mut i = 0;
    while i < t.len() {
        if !token(t[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < t.len() && token(t[i]) {
            i += 1;
        }
        let mut end = i;
        while end > start && t[end - 1] == '.' {
            end -= 1;
        }
        let candidate: String = t[start..end].iter().collect();
        let issue = candidate.rsplit_once('#').is_some_and(|(repo, n)| {
            let number = !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit())
                && n.parse::<u64>().is_ok_and(|n| n > 0);
            number
                && (repo.is_empty()
                    || repo
                        .split_once('/')
                        .is_some_and(|(owner, name)| is_repo_part(owner) && is_repo_part(name)))
        });
        if is_bead_id(&candidate) || issue {
            found.push((start, end - start));
        }
    }
    found
}

/// The web addresses in `t`, as (start, length), by xterm's web-links rule:
/// `http(s)://`, then anything but spaces, quotes and brackets, ending on a
/// character that does not end a sentence.
fn urls(t: &[char]) -> Vec<(usize, usize)> {
    let body = |c: char| !c.is_whitespace() && !"\"'!*(){}|\\^<>`".contains(c);
    let end = |c: char| body(c) && !":,.?~[]".contains(c);
    let mut out = Vec::new();
    let mut i = 0;
    while i < t.len() {
        let Some(scheme) = scheme(&t[i..]) else {
            i += 1;
            continue;
        };
        let from = i + scheme;
        let mut j = from;
        while j < t.len() && body(t[j]) {
            j += 1;
        }
        match (from..j).rev().find(|&k| end(t[k])) {
            Some(k) => {
                out.push((i, k + 1 - i));
                i = k + 1;
            }
            None => i += 1,
        }
    }
    out
}

/// The length of the `http://` or `https://` that `t` starts with.
fn scheme(t: &[char]) -> Option<usize> {
    ["http://", "https://", "HTTP://", "HTTPS://"]
        .iter()
        .find(|s| t.len() >= s.len() && s.chars().zip(t).all(|(a, b)| a == *b))
        .map(|s| s.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::screen::Screen;

    fn link(s: &Screen, line: i32, col: usize) -> Option<Link> {
        at(&s.term, Point::new(Line(line), Column(col)))
    }

    fn screen(cols: usize, rows: usize, text: &str) -> Screen {
        let mut s = Screen::new(cols, rows);
        s.feed(text.replace('\n', "\r\n").as_bytes());
        s
    }

    #[test]
    fn the_paths_in_view_are_found_with_their_rows() {
        let s = screen(
            40,
            6,
            "made out/shot.png\nno path here\nand /tmp/a/b/Plan.SVG then\n/x/y.png",
        );
        let paths: Vec<(String, i32)> = in_lines(&s.term, Line(0), Line(2))
            .into_iter()
            .filter_map(|l| match l.target {
                Target::Path(p) => Some((p, l.cells[0].line.0)),
                _ => None,
            })
            .collect();
        assert_eq!(
            paths,
            vec![("out/shot.png".into(), 0), ("/tmp/a/b/Plan.SVG".into(), 2)]
        );
    }

    #[test]
    fn a_url_opens_and_ends_before_its_full_stop() {
        let s = screen(60, 3, "see https://example.com/a?b=1. next");
        let l = link(&s, 0, 10).expect("a link on the address");
        assert_eq!(l.target, Target::Url("https://example.com/a?b=1".into()));
        assert_eq!(l.cells.first(), Some(&Point::new(Line(0), Column(4))));
        assert_eq!(l.cells.len(), 25);
        assert_eq!(link(&s, 0, 2), None);
        // The path inside an address is the address's.
        assert!(matches!(link(&s, 0, 24).unwrap().target, Target::Url(_)));
    }

    #[test]
    fn a_url_the_terminal_wrapped_is_one_link() {
        let s = screen(20, 3, "go https://example.com/long/path ok");
        let want = Target::Url("https://example.com/long/path".into());
        assert_eq!(link(&s, 0, 5).unwrap().target, want);
        assert_eq!(link(&s, 1, 3).unwrap().target, want);
    }

    #[test]
    fn a_path_opens_without_its_line_number_or_punctuation() {
        let s = screen(60, 2, "wrote /srv/parvi/src/main.rs:12, done");
        let l = link(&s, 0, 8).unwrap();
        assert_eq!(l.target, Target::Path("/srv/parvi/src/main.rs".into()));
        assert_eq!(link(&s, 0, 3), None);
        // A lone slash, or a word with slashes but no file in it, is not a link.
        let s = screen(60, 2, "a/b and / alone");
        assert_eq!(link(&s, 0, 1), None);
        assert_eq!(link(&s, 0, 8), None);
    }

    #[test]
    fn a_file_named_relative_to_the_pane_opens() {
        let s = screen(
            80,
            2,
            "edited src/ui/rail.rs:12 and notes.txt. see ./run and v2.1.3 e.g. x",
        );
        let at = |col| link(&s, 0, col).map(|l| l.target);
        assert_eq!(at(9), Some(Target::Path("src/ui/rail.rs".into())));
        assert_eq!(at(31), Some(Target::Path("notes.txt".into())));
        assert_eq!(at(47), Some(Target::Path("./run".into())));
        // A version, an abbreviation and a plain word are not files.
        assert_eq!(at(56), None);
        assert_eq!(at(63), None);
        assert_eq!(at(0), None);
    }

    #[test]
    fn a_path_after_wide_characters_is_found_at_its_cells() {
        let s = screen(40, 2, "日本 👍 /tmp/x.txt");
        // 日本 takes four cells, the space one, 👍 two, the space one.
        let l = link(&s, 0, 8).expect("the path starts at column 8");
        assert_eq!(l.target, Target::Path("/tmp/x.txt".into()));
        assert_eq!(l.cells.first(), Some(&Point::new(Line(0), Column(8))));
        assert_eq!(l.cells.last(), Some(&Point::new(Line(0), Column(17))));
        // The right half of a wide character is that character's cell.
        assert_eq!(link(&s, 0, 1), None);
    }

    #[test]
    fn a_quoted_path_broken_at_the_margin_opens_whole() {
        // Claude Code breaks a long line itself and indents what follows.
        let s = screen(20, 4, "  Read(\"/srv/parvi/c\n  rates/app/x.rs\")\n");
        let want = Target::Path("/srv/parvi/crates/app/x.rs".into());
        assert_eq!(link(&s, 0, 10).unwrap().target, want);
        let l = link(&s, 1, 4).unwrap();
        assert_eq!(l.target, want);
        assert_eq!(l.cells.len(), want_len(&want));
        // The indentation is not part of it.
        assert_eq!(link(&s, 1, 0), None);
    }

    #[test]
    fn a_hard_break_not_at_the_margin_is_not_joined() {
        let s = screen(30, 4, "  Read(\"/srv/parvi/c\n  rates/app/x.rs\")\n");
        assert_eq!(
            link(&s, 0, 10).unwrap().target,
            Target::Path("/srv/parvi/c".into())
        );
        assert_eq!(link(&s, 1, 4), None);
    }

    #[test]
    fn a_path_the_terminal_wrapped_is_one_link() {
        let s = screen(12, 3, "x /home/user/notes.md");
        let want = Target::Path("/home/user/notes.md".into());
        assert_eq!(link(&s, 0, 4).unwrap().target, want);
        assert_eq!(link(&s, 1, 2).unwrap().target, want);
    }

    #[test]
    fn work_item_ids_and_issues_are_links_including_soft_wraps() {
        let s = screen(
            80,
            3,
            "parvi-abc parvi-pnu.7.3 #42 octo/repo#42 word /srv/parvi-abc",
        );
        for (col, id) in [
            (0, "parvi-abc"),
            (11, "parvi-pnu.7.3"),
            (25, "#42"),
            (30, "octo/repo#42"),
        ] {
            assert_eq!(
                link(&s, 0, col).unwrap().target,
                Target::WorkItem(id.into())
            );
        }
        assert!(link(&s, 0, 43).is_none());
        assert!(matches!(link(&s, 0, 50).unwrap().target, Target::Path(_)));
        let s = screen(12, 3, "x parvi-pnu.7.3");
        let want = Target::WorkItem("parvi-pnu.7.3".into());
        assert_eq!(link(&s, 0, 4).unwrap().target, want);
        assert_eq!(link(&s, 1, 1).unwrap().target, want);
    }

    fn want_len(t: &Target) -> usize {
        match t {
            Target::Path(p) | Target::Url(p) | Target::WorkItem(p) => p.chars().count(),
        }
    }
}
