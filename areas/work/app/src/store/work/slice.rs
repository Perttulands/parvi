//! The slice (CHROTE's `filterSlice` and `orderedRows`): search, project,
//! status and a range of days, the rail's counts, the strip's counts per
//! day, and the table's order.

use super::{Board, Row, STATUSES, Status};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Id,
    Type,
    Title,
    Project,
    Status,
    Updated,
    Idle,
    Size,
    Kids,
}

pub const COLUMNS: [Column; 9] = [
    Column::Id,
    Column::Type,
    Column::Title,
    Column::Project,
    Column::Status,
    Column::Updated,
    Column::Idle,
    Column::Size,
    Column::Kids,
];

impl Column {
    pub fn label(self) -> &'static str {
        match self {
            Column::Id => "ID",
            Column::Type => "TYPE",
            Column::Title => "TITLE",
            Column::Project => "PROJECT",
            Column::Status => "STATUS",
            Column::Updated => "UPDATED",
            Column::Idle => "IDLE",
            Column::Size => "SIZE",
            Column::Kids => "KIDS",
        }
    }

    /// The direction of a first click: text A to Z, times and counts
    /// largest first.
    pub fn natural_descending(self) -> bool {
        matches!(
            self,
            Column::Updated | Column::Idle | Column::Size | Column::Kids
        )
    }
}

/// The slice: the search, the rail's choices, the sort. `None` keeps all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    pub query: String,
    /// Projects kept, by path.
    pub projects: Option<BTreeSet<String>>,
    pub statuses: Option<BTreeSet<Status>>,
    /// Local days, inclusive: a row is in range when it was created,
    /// updated or closed inside it.
    pub range: Option<(i32, i32)>,
    /// The strip spans every day, not only the recent ones.
    pub all_days: bool,
    pub sort: Column,
    pub descending: bool,
}

impl Default for Slice {
    fn default() -> Self {
        Slice {
            query: String::new(),
            projects: None,
            statuses: None,
            range: None,
            all_days: false,
            sort: Column::Updated,
            descending: true,
        }
    }
}

/// What the slice lets through, and the rail's counts: each project's
/// under the status choice, each status's under the project choice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sliced {
    pub rows: Vec<usize>,
    pub project_counts: Vec<usize>,
    pub status_counts: [usize; 5],
    /// Per day from the board's first to today, for the rows every filter
    /// but the range lets through: created, closed, and open at the day's
    /// end, each Bead counted open only from its own creation.
    pub created: Vec<u32>,
    pub closed: Vec<u32>,
    pub open: Vec<u32>,
}

impl Slice {
    pub fn sort_by(&mut self, c: Column) {
        if self.sort == c {
            self.descending = !self.descending;
        } else {
            self.sort = c;
            self.descending = c.natural_descending();
        }
    }

    /// One row toggled; every row kept reads as no filter.
    pub fn toggle_project(&mut self, board: &Board, path: &str) {
        let all: BTreeSet<String> = board.projects.iter().map(|p| p.path.clone()).collect();
        let mut kept = self.projects.clone().unwrap_or_else(|| all.clone());
        if !kept.remove(path) {
            kept.insert(path.to_string());
        }
        self.projects = (kept != all).then_some(kept);
    }

    /// One project alone, or every project when it already stood alone.
    pub fn only_project(&mut self, path: &str) {
        let alone = self
            .projects
            .as_ref()
            .is_some_and(|k| k.len() == 1 && k.contains(path));
        self.projects = (!alone).then(|| BTreeSet::from([path.to_string()]));
    }

    pub fn toggle_status(&mut self, s: Status) {
        let all: BTreeSet<Status> = STATUSES.into_iter().collect();
        let mut kept = self.statuses.clone().unwrap_or_else(|| all.clone());
        if !kept.remove(&s) {
            kept.insert(s);
        }
        self.statuses = (kept != all).then_some(kept);
    }

    pub fn only_status(&mut self, s: Status) {
        let alone = self
            .statuses
            .as_ref()
            .is_some_and(|k| k.len() == 1 && k.contains(&s));
        self.statuses = (!alone).then(|| BTreeSet::from([s]));
    }

    pub fn keeps_project(&self, path: &str) -> bool {
        self.projects.as_ref().is_none_or(|k| k.contains(path))
    }

    pub fn keeps_status(&self, s: Status) -> bool {
        self.statuses.as_ref().is_none_or(|k| k.contains(&s))
    }

    /// Applies the slice to the board, sorted.
    pub fn apply(&self, board: &Board) -> Sliced {
        let terms: Vec<String> = self
            .query
            .to_lowercase()
            .split_whitespace()
            .map(|t| t.trim_start_matches('#').to_string())
            .filter(|t| !t.is_empty())
            .collect();
        let d = &board.days;
        let days = (d.today - d.first + 1).max(1) as usize;
        let mut out = Sliced {
            rows: vec![],
            project_counts: vec![0; board.projects.len()],
            status_counts: [0; 5],
            created: vec![0; days],
            closed: vec![0; days],
            open: vec![0; days],
        };
        let mut delta = vec![0i64; days + 1];
        for (i, r) in board.rows.iter().enumerate() {
            if !terms.iter().all(|t| r.haystack.contains(t.as_str())) {
                continue;
            }
            let p_ok = self.keeps_project(&board.projects[r.project].path);
            let s_ok = self.keeps_status(r.status);
            if p_ok && s_ok {
                // A creation after today (a clock ahead) stays off the strip.
                let created = d.created.get(i).copied().flatten();
                if let Some(c) = created.filter(|&c| c <= d.today) {
                    let c = (c - d.first) as usize;
                    out.created[c] += 1;
                    delta[c] += 1;
                    if let Some(x) = d.closed[i] {
                        // A close stamped before its creation closes on
                        // the creation day.
                        let at = ((x - d.first) as usize).max(c);
                        if at < days {
                            out.closed[at] += 1;
                        }
                        delta[at.min(days)] -= 1;
                    }
                }
            }
            if self
                .range
                .is_some_and(|(from, to)| !d.in_range(i, from, to))
            {
                continue;
            }
            if s_ok {
                out.project_counts[r.project] += 1;
            }
            if p_ok {
                out.status_counts[r.status as usize] += 1;
            }
            if p_ok && s_ok {
                out.rows.push(i);
            }
        }
        let mut running = 0i64;
        for (day, open) in out.open.iter_mut().enumerate() {
            running += delta[day];
            *open = running.max(0) as u32;
        }
        let rows = &board.rows;
        let projects = &board.projects;
        let kids = |i: usize| board.links.children.get(i).map_or(0, Vec::len);
        let idle = |i: usize| d.idle.get(i).copied().flatten();
        // CHROTE's order: the column, ties by id, the whole reversed when
        // descending; status ties go to the most recently updated.
        out.rows.sort_by(|&a, &b| {
            let (x, y): (&Row, &Row) = (&rows[a], &rows[b]);
            let by_id = || natural(&x.id, &y.id);
            let o = match self.sort {
                Column::Id => by_id(),
                Column::Type => x.kind.cmp(&y.kind).then_with(by_id),
                Column::Title => x
                    .title
                    .to_lowercase()
                    .cmp(&y.title.to_lowercase())
                    .then_with(by_id),
                Column::Project => projects[x.project]
                    .label
                    .cmp(&projects[y.project].label)
                    .then_with(by_id),
                Column::Status => x
                    .status
                    .cmp(&y.status)
                    .then_with(|| y.updated.cmp(&x.updated))
                    .then_with(by_id),
                Column::Updated => x.updated.cmp(&y.updated).then_with(by_id),
                Column::Idle => idle(a).cmp(&idle(b)).then_with(by_id),
                Column::Size => x.size.cmp(&y.size).then_with(by_id),
                Column::Kids => kids(a).cmp(&kids(b)).then_with(by_id),
            };
            if self.descending { o.reverse() } else { o }
        });
        out
    }
}

/// Ids in natural order: `ctx-9` before `ctx-10`, `#9` before `#10`.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    crate::natural::natural(a, b)
}
