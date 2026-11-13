//! Mermaid diagrams in a pane's text, found by shape: a fenced block
//! (```` ```mermaid ```` to ```` ``` ````), or a diagram's header line
//! (`flowchart LR`, `sequenceDiagram`, …) followed by lines indented under
//! it, as Claude Code shows a mermaid code block without its fences.

/// A diagram found: where its header is in the text, and its source.
#[derive(Debug, PartialEq, Eq)]
pub struct Block {
    pub at: usize,
    pub len: usize,
    pub source: String,
}

/// The diagram kinds the app draws.
const KINDS: &[&str] = &[
    "flowchart",
    "graph",
    "sequenceDiagram",
    "stateDiagram",
    "stateDiagram-v2",
    "classDiagram",
    "erDiagram",
    "gantt",
    "pie",
    "mindmap",
    "gitGraph",
    "journey",
    "timeline",
];

const DIRECTIONS: &[&str] = &["TD", "TB", "BT", "RL", "LR"];

/// Each line's start and end in `t`, split at `\n`.
fn lines(t: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in t.iter().enumerate() {
        if *c == '\n' {
            out.push((start, i));
            start = i + 1;
        }
    }
    out.push((start, t.len()));
    out
}

fn indent(line: &[char]) -> usize {
    line.iter().take_while(|c| **c == ' ').count()
}

fn blank(line: &[char]) -> bool {
    line.iter().all(|c| c.is_whitespace())
}

/// A diagram's header: its kind first, and for a flowchart a direction
/// or nothing after it.
fn header(text: &str) -> bool {
    let mut words = text.split_whitespace();
    let Some(kind) = words.next() else {
        return false;
    };
    if !KINDS.contains(&kind) {
        return false;
    }
    match kind {
        "flowchart" | "graph" => words.next().is_none_or(|d| DIRECTIONS.contains(&d)),
        _ => true,
    }
}

/// `line` without the first `n` columns of its indentation.
fn dedent(line: &[char], n: usize) -> String {
    line[indent(line).min(n)..]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_string()
}

pub fn blocks(t: &[char]) -> Vec<Block> {
    let rows = lines(t);
    let row = |i: usize| &t[rows[i].0..rows[i].1];
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let line = row(i);
        let n = indent(line);
        let text: String = line[n..].iter().collect::<String>().trim_end().to_string();
        let at = rows[i].0 + n;
        let len = text.chars().count();
        if matches!(text.as_str(), "```mermaid" | "~~~mermaid") {
            let fence = &text[..3];
            let Some(close) = (i + 1..rows.len()).find(|&j| dedent(row(j), usize::MAX) == fence)
            else {
                // Cut off by the rows read, or still being written: what
                // follows is its body, not a diagram of its own.
                break;
            };
            let source: Vec<String> = (i + 1..close).map(|j| dedent(row(j), n)).collect();
            out.push(Block {
                at,
                len,
                source: source.join("\n"),
            });
            i = close + 1;
            continue;
        }
        if !header(&text) {
            i += 1;
            continue;
        }
        // The body: lines indented under the header, blank lines between.
        let mut end = i + 1;
        let mut j = i + 1;
        while j < rows.len() {
            if blank(row(j)) {
                j += 1;
                continue;
            }
            if indent(row(j)) <= n {
                break;
            }
            j += 1;
            end = j;
        }
        if end == i + 1 {
            i += 1;
            continue;
        }
        let mut source = vec![text.clone()];
        source.extend((i + 1..end).map(|j| dedent(row(j), n)));
        out.push(Block {
            at,
            len,
            source: source.join("\n"),
        });
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str) -> Vec<(String, String)> {
        let t: Vec<char> = text.chars().collect();
        blocks(&t)
            .into_iter()
            .map(|b| (t[b.at..b.at + b.len].iter().collect(), b.source))
            .collect()
    }

    #[test]
    fn a_block_claude_code_shows_without_fences_is_found_by_its_header() {
        let text = "\u{25cf} mermaid\n  flowchart LR\n      A[Checkout] --> B[Build]\n\n      B --> C[Test]\n\n  That is the pipeline.";
        assert_eq!(
            found(text),
            vec![(
                "flowchart LR".into(),
                "flowchart LR\n    A[Checkout] --> B[Build]\n\n    B --> C[Test]".into()
            )]
        );
    }

    #[test]
    fn a_fenced_block_is_found_whole_and_an_open_one_is_not() {
        let text = "  ```mermaid\n  sequenceDiagram\n  A->>B: hi\n  ```\nafter";
        assert_eq!(
            found(text),
            vec![("```mermaid".into(), "sequenceDiagram\nA->>B: hi".into())]
        );
        assert!(found("```mermaid\ngraph TD\n  A-->B").is_empty());
    }

    #[test]
    fn prose_that_starts_with_a_kind_word_is_not_a_diagram() {
        assert!(found("graph theory is fun\n  and so on").is_empty());
        assert!(found("pie").is_empty(), "a header with nothing under it");
        assert!(found("timeline of events:\nnext line").is_empty());
    }
}
