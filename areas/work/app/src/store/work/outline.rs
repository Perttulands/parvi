//! What the Work page lists (DESIGN.md, Work): one project's open work as a
//! tree of epics and their items, or, while a search is typed, what matches
//! it, the project's own first and the whole store after it.

use super::{Board, Status};
use std::cmp::Ordering;
use std::collections::HashSet;

/// A line of the tree: a row of the board and how deep it stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub row: usize,
    pub depth: usize,
}

/// Whether a row is work still to do: open, under way or blocked. Deferred
/// and finished work waits for a search.
pub fn open(board: &Board, row: usize) -> bool {
    matches!(
        board.rows[row].status,
        Status::Open | Status::InProgress | Status::Blocked
    )
}

/// Under way first, then ready, then blocked.
fn rank(s: Status) -> u8 {
    match s {
        Status::InProgress => 0,
        Status::Open => 1,
        Status::Blocked => 2,
        Status::Deferred => 3,
        Status::Closed => 4,
    }
}

/// `project`'s open work as a tree. A root is open work whose parent is
/// not open work of this project; under it its open children from any
/// store. Siblings go
/// under way first, then ready, then blocked; roots then by priority and
/// the latest update, children by their ids, as they were planned.
pub fn tree(board: &Board, project: usize) -> Vec<Line> {
    let children = |i: usize| -> Vec<usize> {
        board.links.children[i]
            .iter()
            .copied()
            .filter(|&c| open(board, c))
            .collect()
    };
    // A subtree ranks as its most pressing open row.
    let mut best: Vec<Option<u8>> = vec![None; board.rows.len()];
    fn best_of(
        board: &Board,
        i: usize,
        best: &mut Vec<Option<u8>>,
        seen: &mut HashSet<usize>,
    ) -> u8 {
        if let Some(b) = best[i] {
            return b;
        }
        let mut b = rank(board.rows[i].status);
        if seen.insert(i) {
            for &c in &board.links.children[i] {
                if open(board, c) {
                    b = b.min(best_of(board, c, best, seen));
                }
            }
        }
        best[i] = Some(b);
        b
    }
    let mut seen = HashSet::new();
    let mut roots: Vec<usize> = (0..board.rows.len())
        .filter(|&i| board.rows[i].project == project && open(board, i))
        .filter(|&i| {
            board.links.parent[i]
                .is_none_or(|p| !open(board, p) || board.rows[p].project != project)
        })
        .collect();
    for &r in &roots {
        best_of(board, r, &mut best, &mut seen);
    }
    let by = |a: usize, b: usize, best: &[Option<u8>]| -> (u8, u8) {
        (
            best[a].unwrap_or_else(|| rank(board.rows[a].status)),
            best[b].unwrap_or_else(|| rank(board.rows[b].status)),
        )
    };
    roots.sort_by(|&a, &b| {
        let (x, y) = by(a, b, &best);
        let (ra, rb) = (&board.rows[a], &board.rows[b]);
        x.cmp(&y)
            .then(
                ra.priority
                    .unwrap_or(u8::MAX)
                    .cmp(&rb.priority.unwrap_or(u8::MAX)),
            )
            .then(rb.updated.cmp(&ra.updated))
            .then_with(|| crate::natural::natural(&ra.id, &rb.id))
    });
    let mut out = Vec::new();
    let mut placed = HashSet::new();
    let mut stack: Vec<Line> = roots
        .into_iter()
        .rev()
        .map(|row| Line { row, depth: 0 })
        .collect();
    while let Some(line) = stack.pop() {
        if !placed.insert(line.row) {
            continue;
        }
        out.push(line);
        let mut kids = children(line.row);
        for &k in &kids {
            best_of(board, k, &mut best, &mut seen);
        }
        kids.sort_by(|&a, &b| {
            let (x, y) = by(a, b, &best);
            x.cmp(&y)
                .then_with(|| crate::natural::natural(&board.rows[a].id, &board.rows[b].id))
        });
        for k in kids.into_iter().rev() {
            stack.push(Line {
                row: k,
                depth: line.depth + 1,
            });
        }
    }
    out
}

/// What a search finds: in the chosen project, then in every other.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub here: Vec<usize>,
    pub elsewhere: Vec<usize>,
    /// Matches left out past [`SHOWN`] in either.
    pub more: usize,
}

/// The most matches listed from each side of a search.
pub const SHOWN: usize = 100;

/// The rows holding every word of `query` in their id, title, text or
/// labels: `project`'s first, open work before finished, each the latest
/// update first.
pub fn search(board: &Board, project: Option<usize>, query: &str) -> Found {
    let terms: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(|t| t.trim_start_matches('#').to_string())
        .filter(|t| !t.is_empty())
        .collect();
    let mut found = Found::default();
    if terms.is_empty() {
        return found;
    }
    for (i, r) in board.rows.iter().enumerate() {
        if terms.iter().all(|t| r.haystack.contains(t.as_str())) {
            if Some(r.project) == project {
                found.here.push(i);
            } else {
                found.elsewhere.push(i);
            }
        }
    }
    let order = |a: &usize, b: &usize| -> Ordering {
        let (x, y) = (&board.rows[*a], &board.rows[*b]);
        rank(x.status)
            .min(3)
            .cmp(&rank(y.status).min(3))
            .then(y.updated.cmp(&x.updated))
    };
    for list in [&mut found.here, &mut found.elsewhere] {
        list.sort_by(order);
        if list.len() > SHOWN {
            found.more += list.len() - SHOWN;
            list.truncate(SHOWN);
        }
    }
    found
}

/// The project a folder is in: the one whose folder holds it most
/// closely, as a session's folder names its project.
pub fn project_of_folder(board: &Board, folder: &str) -> Option<usize> {
    let folder = folder.trim_end_matches('/');
    board
        .projects
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            let home = p.path.trim_end_matches('/');
            folder == home || folder.starts_with(&format!("{home}/"))
        })
        .max_by_key(|(_, p)| p.path.len())
        .map(|(i, _)| i)
}

/// How much open work each project holds.
pub fn open_counts(board: &Board) -> Vec<usize> {
    let mut n = vec![0; board.projects.len()];
    for i in 0..board.rows.len() {
        if open(board, i) {
            n[board.rows[i].project] += 1;
        }
    }
    n
}
