//! The Work column (CHROTE's Beads column, `BeadsColumn.tsx`): what is
//! moving in every Beads store, beside any page. A store's claimed work
//! comes before its ready work, each newest first; a store with neither is
//! left out, and a store that failed is listed so failure never reads as
//! quiet. GitHub states say nothing of claimed or ready, so repositories
//! stay on the Work tab, as CHROTE's column holds only Beads.

use super::{Board, Status, Tracker};

/// One store's moving work, as rows of the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub project: usize,
    pub in_progress: Vec<usize>,
    pub ready: Vec<usize>,
}

/// The column's groups by store label, and the stores that failed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Moving {
    pub groups: Vec<Group>,
    pub failed: Vec<usize>,
}

/// CHROTE's `arrangeBeadsColumnGroups`: in progress, then ready (open, not
/// blocked, not deferred), newest updated first.
pub fn moving(board: &Board) -> Moving {
    let mut m = Moving::default();
    for (p, info) in board.projects.iter().enumerate() {
        if info.tracker != Tracker::Beads {
            continue;
        }
        if info.error.is_some() {
            m.failed.push(p);
        }
        let of = |s: Status| {
            let mut rows: Vec<usize> = (0..board.rows.len())
                .filter(|&i| board.rows[i].project == p && board.rows[i].status == s)
                .collect();
            rows.sort_by(|&a, &b| board.rows[b].updated.cmp(&board.rows[a].updated));
            rows
        };
        let (in_progress, ready) = (of(Status::InProgress), of(Status::Open));
        if !in_progress.is_empty() || !ready.is_empty() {
            m.groups.push(Group {
                project: p,
                in_progress,
                ready,
            });
        }
    }
    m.groups.sort_by(|a, b| {
        board.projects[a.project]
            .label
            .cmp(&board.projects[b.project].label)
    });
    m
}
