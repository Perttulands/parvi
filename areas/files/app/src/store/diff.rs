//! Unified diff rows, with signs kept separate for the gutter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Hunk,
    Add,
    Delete,
    Context,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: Kind,
    pub text: String,
}

pub fn rows(diff: &str) -> Vec<Row> {
    let mut started = false;
    let mut rows = Vec::new();
    for line in diff.lines() {
        let line = line.trim_end_matches('\r');
        if !started {
            if !line.starts_with("@@") {
                continue;
            }
            started = true;
        }
        let (kind, text) = if line.starts_with("@@") {
            (Kind::Hunk, line)
        } else if let Some(line) = line.strip_prefix('+') {
            (Kind::Add, line)
        } else if let Some(line) = line.strip_prefix('-') {
            (Kind::Delete, line)
        } else {
            (Kind::Context, line.strip_prefix(' ').unwrap_or(line))
        };
        rows.push(Row {
            kind,
            text: text.to_string(),
        });
    }
    while rows
        .last()
        .is_some_and(|r| r.kind == Kind::Context && r.text.is_empty())
    {
        rows.pop();
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_hunks_and_signed_lines_without_git_headers() {
        let rows = rows(
            "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n kept\n\\ No newline at end of file\n",
        );
        assert_eq!(
            rows.iter()
                .map(|r| (r.kind, r.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (Kind::Hunk, "@@ -1 +1 @@"),
                (Kind::Delete, "old"),
                (Kind::Add, "new"),
                (Kind::Context, "kept"),
                (Kind::Context, "\\ No newline at end of file")
            ]
        );
        assert!(super::rows("diff --git a/x b/x\n").is_empty());
    }
}
