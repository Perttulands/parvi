//! The work graph beside the table (CHROTE's `workGraph.ts`): the family
//! the selected item belongs to.
//!
//! The family's head is the selected item when it has children, otherwise
//! its parent, otherwise the item itself; so selecting one child after
//! another keeps the same picture and only moves the highlight. The nodes
//! are the head's children (or the lone item), plus every item outside the
//! family that blocks one of them or waits on one, in any store. Edges are
//! the blocking edges among the nodes (for GitHub, a pull request blocks
//! the issue it closes). Layout runs in waves by longest blocking path:
//! across the pane, across in bands one under the next, or down it,
//! whichever draws the family largest in the pane it has. A wave longer
//! than the pane wraps into sub-columns, unfinished work first.

use super::{Board, Status};
use std::collections::{HashMap, HashSet};

pub const NODE_WIDTH: f32 = 196.;
pub const NODE_HEIGHT: f32 = 40.;
/// The smallest scale a graph opens at: below it a card's words stop being read.
pub const READABLE_SCALE: f32 = 0.85;
/// The smallest scale a family is shrunk to so that it is seen whole.
pub const WHOLE_SCALE: f32 = 0.7;
const GAP_X: f32 = 44.;
const GAP_Y: f32 = 10.;
/// Between waves drawn down the pane, and between bands.
const GAP_DOWN: f32 = 30.;
const MOST_BANDS: usize = 4;
const WRAP_GAP_X: f32 = 16.;
const PAD: f32 = 16.;
/// Items outside the family one graph draws at most.
const MAX_RING: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub row: usize,
    pub x: f32,
    pub y: f32,
    /// Outside the family: here because it blocks a member or waits on one.
    pub outside: bool,
}

/// Node indices: `from` blocks `to`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Across,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    pub orientation: Orientation,
    /// The scale the family opens at in the pane it was laid out for.
    pub scale: f32,
    /// The family's head, the pane's title.
    pub head: usize,
    /// The head's own parent, the way up.
    pub head_parent: Option<usize>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub width: f32,
    pub height: f32,
    /// Outside items left out past the cap.
    pub omitted: usize,
}

/// The head of the family a row is shown in.
pub fn family_head(board: &Board, selected: usize) -> usize {
    if !board.links.children[selected].is_empty() {
        return selected;
    }
    board.links.parent[selected].unwrap_or(selected)
}

/// How large a drawing can be shown whole in the pane.
pub fn fit_scale(pane: (f32, f32), width: f32, height: f32) -> f32 {
    1f32.min(pane.0 / width.max(1.))
        .min(pane.1 / height.max(1.))
}

/// Unfinished first, then ids in natural order.
fn order(board: &Board, a: usize, b: usize) -> std::cmp::Ordering {
    let done = |i: usize| board.rows[i].status == Status::Closed;
    done(a)
        .cmp(&done(b))
        .then_with(|| crate::natural::natural(&board.rows[a].id, &board.rows[b].id))
}

/// The family of `selected`, laid out at scale 1 for a pane of
/// `pane` (width, height).
pub fn build(board: &Board, selected: usize, pane: (f32, f32)) -> Graph {
    let links = &board.links;
    let head = family_head(board, selected);
    let members: Vec<usize> = if links.children[head].is_empty() {
        vec![head]
    } else {
        links.children[head].clone()
    };
    let in_family: HashSet<usize> = members.iter().copied().collect();
    let mut ring: Vec<usize> = vec![];
    let mut in_ring: HashSet<usize> = HashSet::new();
    let mut omitted = 0;
    let mut sorted = members.clone();
    sorted.sort_by(|&a, &b| order(board, a, b));
    // Unfinished members first, so a cap drops the context of finished work.
    for m in sorted {
        for &other in links.blockers[m].iter().chain(&links.dependents[m]) {
            if in_family.contains(&other) || in_ring.contains(&other) || other == head {
                continue;
            }
            if ring.len() >= MAX_RING {
                omitted += 1;
                continue;
            }
            in_ring.insert(other);
            ring.push(other);
        }
    }
    let rows: Vec<usize> = members.iter().chain(&ring).copied().collect();
    let node_of: HashMap<usize, usize> = rows.iter().enumerate().map(|(i, &r)| (r, i)).collect();
    let mut edges = vec![];
    for (to, &row) in rows.iter().enumerate() {
        for b in &links.blockers[row] {
            if let Some(&from) = node_of.get(b) {
                edges.push(Edge { from, to });
            }
        }
    }

    // Longest blocking path from any source; a cycle's back edge is ignored.
    let mut incoming: Vec<Vec<usize>> = vec![vec![]; rows.len()];
    for e in &edges {
        incoming[e.to].push(e.from);
    }
    let mut wave: Vec<Option<usize>> = vec![None; rows.len()];
    let mut visiting = vec![false; rows.len()];
    fn depth(
        node: usize,
        incoming: &[Vec<usize>],
        wave: &mut [Option<usize>],
        visiting: &mut [bool],
    ) -> usize {
        if let Some(w) = wave[node] {
            return w;
        }
        if visiting[node] {
            return 0;
        }
        visiting[node] = true;
        let mut deepest = 0;
        for &from in &incoming[node] {
            deepest = deepest.max(depth(from, incoming, wave, visiting) + 1);
        }
        visiting[node] = false;
        wave[node] = Some(deepest);
        deepest
    }
    for node in 0..rows.len() {
        depth(node, &incoming, &mut wave, &mut visiting);
    }
    let mut waves: Vec<Vec<usize>> = vec![];
    for (node, w) in wave.iter().enumerate() {
        let w = w.unwrap_or(0);
        if waves.len() <= w {
            waves.resize(w + 1, vec![]);
        }
        waves[w].push(node);
    }
    let columns: Vec<Vec<usize>> = waves
        .into_iter()
        .filter(|c| !c.is_empty())
        .map(|mut c| {
            c.sort_by(|&a, &b| order(board, rows[a], rows[b]));
            c
        })
        .collect();
    let across = place_across(&columns, pane.1, 1);
    let mut alternatives = vec![place_down(&columns, pane.0)];
    for bands in 2..=MOST_BANDS.min(columns.len()) {
        alternatives.push(place_across(&columns, pane.1, bands));
    }
    // Another shape only when it draws the whole family readable and larger
    // than one band across does; when none is readable whole, whichever is
    // largest if it is whole at WHOLE_SCALE.
    let pick = |floor: f32| {
        let mut chosen: Option<usize> = None;
        let mut best = fit_scale(pane, across.width, across.height);
        for (k, c) in alternatives.iter().enumerate() {
            let fit = fit_scale(pane, c.width, c.height);
            if fit >= floor && fit > best + 0.01 {
                chosen = Some(k);
                best = fit;
            }
        }
        (chosen, best)
    };
    let (mut chosen, mut best) = pick(READABLE_SCALE);
    if best < READABLE_SCALE {
        (chosen, best) = pick(WHOLE_SCALE);
    }
    let placed = chosen.map_or(&across, |k| &alternatives[k]);
    Graph {
        orientation: if chosen == Some(0) {
            Orientation::Down
        } else {
            Orientation::Across
        },
        scale: if best >= WHOLE_SCALE {
            best
        } else {
            READABLE_SCALE
        },
        head,
        head_parent: links.parent[head],
        nodes: rows
            .iter()
            .enumerate()
            .map(|(n, &row)| Node {
                row,
                x: placed.x[n],
                y: placed.y[n],
                outside: in_ring.contains(&row),
            })
            .collect(),
        edges,
        width: placed.width,
        height: placed.height,
        omitted,
    }
}

struct Placement {
    x: Vec<f32>,
    y: Vec<f32>,
    width: f32,
    height: f32,
}

fn placement(waves: &[Vec<usize>]) -> Placement {
    let n = waves.iter().map(Vec::len).sum();
    Placement {
        x: vec![0.; n],
        y: vec![0.; n],
        width: 0.,
        height: 0.,
    }
}

/// Waves as columns, each wrapping into sub-columns as tall as the pane,
/// in `bands` bands of about equal width, each under the one before.
fn place_across(waves: &[Vec<usize>], pane_height: f32, bands: usize) -> Placement {
    let per_column = (((pane_height / READABLE_SCALE - 2. * PAD + GAP_Y) / (NODE_HEIGHT + GAP_Y))
        .floor() as usize)
        .max(4);
    let span = |c: &Vec<usize>| {
        c.len().div_ceil(per_column) as f32 * (NODE_WIDTH + WRAP_GAP_X) - WRAP_GAP_X
    };
    let target = (waves.iter().map(span).sum::<f32>()
        + waves.len().saturating_sub(1) as f32 * GAP_X)
        / bands as f32;
    let mut p = placement(waves);
    let (mut band, mut x, mut top, mut bottom) = (0, PAD, PAD, PAD);
    for (w, column) in waves.iter().enumerate() {
        if x > PAD
            && band < bands - 1
            && x - PAD + span(column) > target + 1.
            && waves.len() - w >= bands - band - 1
        {
            band += 1;
            x = PAD;
            top = bottom + GAP_DOWN;
        }
        for (i, &node) in column.iter().enumerate() {
            p.x[node] = x + (i / per_column) as f32 * (NODE_WIDTH + WRAP_GAP_X);
            p.y[node] = top + (i % per_column) as f32 * (NODE_HEIGHT + GAP_Y);
            bottom = bottom.max(p.y[node] + NODE_HEIGHT);
        }
        x += span(column) + GAP_X;
        p.width = p.width.max(x - GAP_X + PAD);
    }
    p.height = bottom + PAD;
    p
}

/// Waves as rows, each wrapping into sub-rows as wide as the pane, centred.
fn place_down(waves: &[Vec<usize>], pane_width: f32) -> Placement {
    let per_row = (((pane_width / READABLE_SCALE - 2. * PAD + WRAP_GAP_X)
        / (NODE_WIDTH + WRAP_GAP_X))
        .floor() as usize)
        .max(2);
    let mut p = placement(waves);
    let widest = per_row.min(waves.iter().map(Vec::len).max().unwrap_or(1).max(1));
    let span = |n: usize| n as f32 * NODE_WIDTH + n.saturating_sub(1) as f32 * WRAP_GAP_X;
    let mut y = PAD;
    for row in waves {
        let lines = row.len().div_ceil(per_row);
        for (i, &node) in row.iter().enumerate() {
            let line = i / per_row;
            let in_line = per_row.min(row.len() - line * per_row);
            p.x[node] = PAD
                + (span(widest) - span(in_line)) / 2.
                + (i % per_row) as f32 * (NODE_WIDTH + WRAP_GAP_X);
            p.y[node] = y + line as f32 * (NODE_HEIGHT + GAP_Y);
        }
        y += lines as f32 * (NODE_HEIGHT + GAP_Y) - GAP_Y + GAP_DOWN;
    }
    p.width = span(widest) + 2. * PAD;
    p.height = y - GAP_DOWN + PAD;
    p
}

/// The curve from a blocker to what it blocks, as start, two control
/// points and end: right edge to left edge across the pane, bottom edge to
/// top edge down it, and down too from one band's end to the next band.
pub fn edge_path(from: &Node, to: &Node, orientation: Orientation) -> [(f32, f32); 4] {
    let next_band = to.y >= from.y + NODE_HEIGHT && to.x < from.x + NODE_WIDTH;
    if orientation == Orientation::Down || next_band {
        let (x1, y1) = (from.x + NODE_WIDTH / 2., from.y + NODE_HEIGHT);
        let (x2, y2) = (to.x + NODE_WIDTH / 2., to.y);
        let bend = 12f32.max((y2 - y1).abs() / 2.);
        return [(x1, y1), (x1, y1 + bend), (x2, y2 - bend), (x2, y2)];
    }
    let (x1, y1) = (from.x + NODE_WIDTH, from.y + NODE_HEIGHT / 2.);
    let (x2, y2) = (to.x, to.y + NODE_HEIGHT / 2.);
    let bend = 16f32.max((x2 - x1).abs() / 2.);
    [(x1, y1), (x1 + bend, y1), (x2 - bend, y2), (x2, y2)]
}

/// FNV-1a over UTF-16 code units, as CHROTE hashes a key.
fn hash(text: &str) -> u32 {
    let mut v: u32 = 2_166_136_261;
    for c in text.encode_utf16() {
        v ^= u32::from(c);
        v = v.wrapping_mul(16_777_619);
    }
    v
}

/// Shelf colours for the projects one graph shows: each project's own
/// colour by its key, moved to the next free one only when another project
/// in the same graph already holds it.
pub fn project_colours(keys: &[&str], shelves: usize) -> HashMap<String, usize> {
    let mut keys: Vec<&str> = keys.to_vec();
    keys.sort();
    keys.dedup();
    let mut colours = HashMap::new();
    let mut taken = HashSet::new();
    for key in keys {
        let mut c = hash(key) as usize % shelves;
        for _ in 0..shelves {
            if !taken.contains(&c) {
                break;
            }
            c = (c + 1) % shelves;
        }
        taken.insert(c);
        colours.insert(key.to_string(), c);
    }
    colours
}
