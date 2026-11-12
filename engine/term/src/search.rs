//! Searching every pane's scrollback (issue #22). Parvi holds no
//! scrollback of its own, so a search reads tmux's history on demand with
//! `capture-pane -p -J -S -`. Captures run independently of the sessions'
//! control clients: tmux uses a separate process and the holder a separate
//! connection, so a long capture does not hold up live output.
//!
//! The cost is bounded: at most [`CONCURRENT`] captures at once, a time
//! budget per search, and a cap on matches. A capture is read line by line
//! as the backend writes it, keeping only the line before, so no history is ever
//! held whole. Nothing is kept afterwards.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::AsyncBufReadExt;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use crate::backend::Backend;
use crate::control::PaneId;

/// Captures running at once.
pub const CONCURRENT: usize = 4;

/// How long one search may run before it stops with what it found.
pub const BUDGET: Duration = Duration::from_secs(10);

/// The most matches one search sends.
pub const MAX_MATCHES: usize = 500;

/// The longest line a match carries, in characters.
const MAX_LINE: usize = 400;

/// A line that matched, with the lines either side of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub pane: PaneId,
    /// The line's index in the pane's capture, from the oldest history line.
    pub line: u32,
    /// The line before, the line, and the line after; empty at the edges.
    pub context: [String; 3],
}

/// A pane to search: tmux's id for it on its server, and the id its matches
/// carry, which differ for another user's server (see [`crate::hub`]).
#[derive(Clone)]
pub struct Target {
    pub backend: Arc<dyn Backend>,
    pub pane: PaneId,
    pub id: PaneId,
}

impl Target {
    /// A pane whose matches carry tmux's own id.
    pub fn own(backend: &(impl Backend + Clone), pane: PaneId) -> Target {
        Target {
            backend: Arc::new(backend.clone()),
            pane,
            id: pane,
        }
    }
}

/// How a search ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub matches: u32,
    pub panes: u32,
    /// Stopped early: the match cap or the time budget.
    pub truncated: bool,
}

/// Searches each pane's history and screen for `query`, sending matches as
/// each pane's capture is read. Case matters only if the query has a
/// capital letter. Panes that can't be captured, such as ones that went,
/// are skipped.
pub async fn search(panes: Vec<Target>, query: &str, matches: mpsc::Sender<Match>) -> Summary {
    let needle = Arc::new(Needle::new(query));
    let limit = Arc::new(Semaphore::new(CONCURRENT));
    let read = Arc::new(AtomicU32::new(0));
    let (found, mut found_rx) = mpsc::channel(64);
    // Dropped on return, which cancels every scan and its backend command.
    let mut scans = JoinSet::new();
    for target in panes {
        let (limit, needle, read, found) =
            (limit.clone(), needle.clone(), read.clone(), found.clone());
        scans.spawn(async move {
            let Ok(_permit) = limit.acquire_owned().await else {
                return;
            };
            if scan(&target, &needle, &found).await.is_ok() {
                read.fetch_add(1, Ordering::Relaxed);
            }
        });
    }
    drop(found);
    let mut summary = Summary::default();
    let deadline = tokio::time::Instant::now() + BUDGET;
    loop {
        let next = match tokio::time::timeout_at(deadline, found_rx.recv()).await {
            Ok(Some(next)) => next,
            // Every scan is over.
            Ok(None) => break,
            Err(_) => {
                summary.truncated = true;
                break;
            }
        };
        if summary.matches as usize >= MAX_MATCHES {
            summary.truncated = true;
            break;
        }
        if matches.send(next).await.is_err() {
            break;
        }
        summary.matches += 1;
    }
    summary.panes = read.load(Ordering::Relaxed);
    summary
}

/// Reads one pane's capture as tmux writes it and sends each matching line
/// with the lines either side. Fails if the pane can't be captured.
async fn scan(target: &Target, needle: &Needle, found: &mpsc::Sender<Match>) -> Result<()> {
    let pane = format!("%{}", target.pane);
    let mut stream = target
        .backend
        .stream(&["capture-pane", "-p", "-J", "-S", "-", "-t", &pane])
        .await?;
    let (mut line, mut before) = (Vec::new(), Vec::new());
    // A match waiting for the line after it.
    let mut waiting: Option<Match> = None;
    let mut index = 0u32;
    loop {
        line.clear();
        if stream.reader.read_until(b'\n', &mut line).await? == 0 {
            break;
        }
        while line.last().is_some_and(|&b| b == b'\n' || b == b'\r') {
            line.pop();
        }
        let text = String::from_utf8_lossy(&line);
        if let Some(mut m) = waiting.take() {
            m.context[2] = clip(&text);
            if found.send(m).await.is_err() {
                return Ok(());
            }
        }
        if let Some(at) = needle.find(&text) {
            waiting = Some(Match {
                pane: target.id,
                line: index,
                context: [
                    clip(&String::from_utf8_lossy(&before)),
                    clip_around(&text, at),
                    String::new(),
                ],
            });
        }
        std::mem::swap(&mut line, &mut before);
        index += 1;
    }
    if let Some(m) = waiting
        && found.send(m).await.is_err()
    {
        return Ok(());
    }
    (&mut stream.completion).await??;
    Ok(())
}

/// A pane's whole history and screen, one line per row with wrapped lines
/// joined, and colours and attributes as escapes. The same rows as a search
/// reads, so a match's line index finds it here.
pub async fn capture(backend: &dyn Backend, pane: PaneId) -> Result<String> {
    let target = format!("%{pane}");
    tokio::time::timeout(
        BUDGET,
        backend.command(&["capture-pane", "-p", "-e", "-J", "-S", "-", "-t", &target]),
    )
    .await
    .context("capture-pane took too long")?
    .context("capture-pane")
}

fn clip(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

/// A long line cut to [`MAX_LINE`] characters with the hit, at character
/// `at`, kept in: from a little before it, marked `…` where cut.
fn clip_around(line: &str, at: usize) -> String {
    let from = at.saturating_sub(MAX_LINE / 4);
    if from == 0 {
        return clip(line);
    }
    let rest: String = line.chars().skip(from).collect();
    format!("…{}", clip(&rest))
}

/// Smart case: a query with a capital letter matches case and all.
struct Needle {
    text: String,
    exact: bool,
}

impl Needle {
    fn new(query: &str) -> Needle {
        let exact = query.chars().any(char::is_uppercase);
        Needle {
            text: if exact {
                query.to_owned()
            } else {
                query.to_lowercase()
            },
            exact,
        }
    }

    #[cfg(test)]
    fn matches(&self, line: &str) -> bool {
        self.find(line).is_some()
    }

    /// The character the first hit in `line` starts at.
    fn find(&self, line: &str) -> Option<usize> {
        if self.text.is_empty() {
            return None;
        }
        let byte = if self.exact {
            line.find(&self.text)?
        } else {
            line.to_lowercase().find(&self.text)?
        };
        let lowered = (!self.exact).then(|| line.to_lowercase());
        Some(lowered.as_deref().unwrap_or(line)[..byte].chars().count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_with_smart_case() {
        let any = Needle::new("error");
        assert!(any.matches("An ERROR here"));
        let exact = Needle::new("Error");
        assert!(exact.matches("Error: x"));
        assert!(!exact.matches("error: x"));
        assert!(!Needle::new("").matches("anything"));
    }

    #[test]
    fn clips_long_lines() {
        let long = "é".repeat(MAX_LINE + 10);
        assert_eq!(clip(&long).chars().count(), MAX_LINE + 1);
        assert_eq!(clip("short"), "short");
    }

    #[test]
    fn a_long_line_keeps_its_hit() {
        let line = format!("{}needle{}", "x".repeat(MAX_LINE * 2), "y".repeat(50));
        let at = Needle::new("needle").find(&line).unwrap();
        assert_eq!(at, MAX_LINE * 2);
        let kept = clip_around(&line, at);
        assert!(kept.starts_with('…') && kept.contains("needle"), "{kept}");
        assert_eq!(clip_around("short needle", 6), "short needle");
    }
}
