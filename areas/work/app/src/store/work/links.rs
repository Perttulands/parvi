//! The links between rows across every store and repository (CHROTE's
//! `buildSliceIndex`): a Bead's parent and the children filed under it from
//! any store, what it waits on and what waits on it. The node names a Bead
//! of its own store canonically and one of another store by its bare id; a
//! bare id resolves in its own store first, else to the one store that holds
//! it. GitHub's links are the same edges: a pull request blocks the issues
//! it closes.

use super::{Board, Tracker};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Links {
    pub parent: Vec<Option<usize>>,
    pub children: Vec<Vec<usize>>,
    /// What each row waits on.
    pub blockers: Vec<Vec<usize>>,
    /// What waits on each row.
    pub dependents: Vec<Vec<usize>>,
    /// Blocker ids no store holds: unfinished, as far as anyone can tell.
    pub unresolved: Vec<Vec<String>>,
}

impl Links {
    pub fn of(board: &Board) -> Links {
        let n = board.rows.len();
        let mut l = Links {
            parent: vec![None; n],
            children: vec![vec![]; n],
            blockers: vec![vec![]; n],
            dependents: vec![vec![]; n],
            unresolved: vec![vec![]; n],
        };
        for (i, r) in board.rows.iter().enumerate() {
            let canonical = |m: &str| m.contains('#').then(|| board.find(m)).flatten();
            if let Some(p) = r.family.parent.as_deref() {
                let j = canonical(p).or_else(|| board.find_from(p, r.project));
                if let Some(j) = j.filter(|&j| j != i) {
                    l.parent[i] = Some(j);
                    l.children[j].push(i);
                }
            }
            let waits_on = match board.projects[r.project].tracker {
                Tracker::Beads => &r.family.blockers,
                Tracker::Github => &r.family.pulls,
            };
            for m in waits_on {
                // An id two other stores hold could name either: it waits
                // on both, so it is finished only when both are.
                let named: Vec<usize> = if m.contains('#') {
                    board.find(m).into_iter().collect()
                } else if let Some(j) = board.find_from(m, r.project) {
                    vec![j]
                } else {
                    board.holders(m).to_vec()
                };
                if named.is_empty() {
                    l.unresolved[i].push(m.clone());
                }
                for j in named.into_iter().filter(|&j| j != i) {
                    l.blockers[i].push(j);
                    l.dependents[j].push(i);
                }
            }
        }
        l
    }
}
