//! Deterministic compact reading order for a mission canvas.
use parvi_mission_model::{Layout, LayoutNode, Mission};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone)]
struct Item {
    kind: String,
    files: bool,
}

/// Computes positions without saving them; persisted wire lanes are retained.
pub fn arrange(mission: &Mission, layout: &Layout) -> Layout {
    let mut items = BTreeMap::<String, Item>::new();
    let mut inputs = Vec::new();
    let mut gates = Vec::new();
    for n in &mission.input_cards {
        inputs.push(n.id.clone());
        items.insert(
            n.id.clone(),
            Item {
                kind: "inputCard".into(),
                files: !n.files.is_empty(),
            },
        );
    }
    for n in &mission.formations {
        items.insert(
            n.id.clone(),
            Item {
                kind: n.kind.clone(),
                files: n.brief.as_ref().is_some_and(|b| !b.files.is_empty()),
            },
        );
    }
    for n in &mission.gates {
        gates.push(n.id.clone());
        let files = !n.files.is_empty()
            || crate::graph::judge_chain(mission, &n.id)
                .iter()
                .any(|f| f.brief.as_ref().is_some_and(|b| !b.files.is_empty()));
        items.insert(
            n.id.clone(),
            Item {
                kind: "gate".into(),
                files,
            },
        );
    }
    for n in &mission.ends {
        items.insert(
            n.id.clone(),
            Item {
                kind: "end".into(),
                files: false,
            },
        );
    }
    for n in &mission.limits {
        items.insert(
            n.id.clone(),
            Item {
                kind: "limit".into(),
                files: false,
            },
        );
    }
    inputs.sort();
    gates.sort();
    let mut owner = BTreeMap::<String, String>::new();
    let mut below = BTreeMap::<String, Vec<String>>::new();
    for gate in &gates {
        for judge in crate::graph::judge_chain(mission, gate) {
            if !owner.contains_key(&judge.id) {
                owner.insert(judge.id.clone(), gate.clone());
                below
                    .entry(gate.clone())
                    .or_default()
                    .push(judge.id.clone());
            }
        }
    }
    for n in &mission.limits {
        if mission.formation(&n.target).is_some() || mission.has_input_card(&n.target) {
            owner.insert(n.id.clone(), n.target.clone());
            below
                .entry(n.target.clone())
                .or_default()
                .push(n.id.clone());
        }
    }
    let mut forward = BTreeMap::<String, Vec<String>>::new();
    let mut fails = BTreeMap::<String, Vec<String>>::new();
    for edge in &mission.connections {
        let Some((from, from_port)) = crate::graph::split_endpoint(&edge.from) else {
            continue;
        };
        let Some((to, to_port)) = crate::graph::split_endpoint(&edge.to) else {
            continue;
        };
        if from == to
            || !items.contains_key(from)
            || !items.contains_key(to)
            || owner.contains_key(from)
            || owner.contains_key(to)
        {
            continue;
        }
        let from_gate = mission.gate(from).is_some();
        let to_gate = mission.gate(to).is_some();
        if (from_gate && from_port == "judge") || (to_gate && to_port == "judge") {
            continue;
        }
        let map = if from_gate && from_port == "fail" {
            &mut fails
        } else {
            &mut forward
        };
        map.entry(from.into()).or_default().push(to.into());
    }
    for list in forward.values_mut().chain(fails.values_mut()) {
        list.sort();
    }
    let mut depths = BTreeMap::new();
    let mut order = Vec::new();
    place(
        &inputs,
        &BTreeMap::new(),
        &forward,
        &owner,
        &mut depths,
        &mut order,
        0,
    );
    loop {
        let mut roots = BTreeMap::new();
        for gate in &gates {
            if let Some(depth) = depths.get(gate) {
                for target in fails.get(gate).into_iter().flatten() {
                    if !depths.contains_key(target) && !owner.contains_key(target) {
                        let entry = roots.entry(target.clone()).or_insert(0);
                        *entry = (*entry).max(depth + 1);
                    }
                }
            }
        }
        if roots.is_empty() {
            break;
        }
        let ids: Vec<_> = roots.keys().cloned().collect();
        place(&ids, &roots, &forward, &owner, &mut depths, &mut order, 0);
    }
    let after = depths.values().max().map_or(0, |d| d + 1);
    loop {
        let mut reached = BTreeSet::new();
        for id in items
            .keys()
            .filter(|id| !depths.contains_key(*id) && !owner.contains_key(*id))
        {
            reached.extend(forward.get(id).into_iter().flatten().cloned());
        }
        let mut roots: Vec<_> = items
            .keys()
            .filter(|id| {
                !depths.contains_key(*id) && !owner.contains_key(*id) && !reached.contains(*id)
            })
            .cloned()
            .collect();
        if roots.is_empty()
            && let Some(id) = items
                .keys()
                .find(|id| !depths.contains_key(*id) && !owner.contains_key(*id))
        {
            roots.push(id.clone());
        }
        if roots.is_empty() {
            break;
        }
        place(
            &roots,
            &BTreeMap::new(),
            &forward,
            &owner,
            &mut depths,
            &mut order,
            after,
        );
    }
    let mut columns = BTreeMap::<usize, Vec<String>>::new();
    for id in order {
        columns.entry(depths[&id]).or_default().push(id);
    }
    let mut nodes = Vec::new();
    let (mut x, mut row_y, mut row_bottom) = (112, 112, 112);
    for (column, ids) in columns.values().enumerate() {
        if column > 0 && column % 4 == 0 {
            x = 112;
            row_y = row_bottom;
        }
        let (mut y, mut width) = (row_y, 0);
        for id in ids {
            let mut stack = Vec::new();
            stacked(id, &below, &mut stack);
            for id in stack {
                let item = &items[&id];
                let (w, h) = size(item);
                nodes.push(LayoutNode { id, x, y });
                y = snap(y + h + 112);
                width = width.max(w);
            }
        }
        row_bottom = row_bottom.max(y);
        x = snap(x + width + 84);
    }
    Layout {
        nodes,
        ..layout.clone()
    }
}

fn stacked(id: &str, below: &BTreeMap<String, Vec<String>>, out: &mut Vec<String>) {
    out.push(id.into());
    for child in below.get(id).into_iter().flatten() {
        stacked(child, below, out);
    }
}
fn size(item: &Item) -> (i64, i64) {
    let (width, height) = match item.kind.as_str() {
        "inputCard" => (260, 168),
        "gate" => (332, 132),
        "end" => (200, 64),
        "limit" => (220, 72),
        "peer" => (336, 360),
        "orchestrated" => (336, 460),
        _ => (320, 320),
    };
    (width, height + 112 + if item.files { 30 } else { 0 })
}
fn snap(n: i64) -> i64 {
    ((n + 27) / 28) * 28
}

type Edges = BTreeMap<String, Vec<String>>;
fn visit(
    id: &str,
    root: &str,
    graph: (&Edges, &BTreeMap<String, usize>, &BTreeMap<String, String>),
    visiting: &mut BTreeSet<String>,
    roots: &mut BTreeMap<String, String>,
    predecessors: &mut Edges,
) {
    roots.insert(id.into(), root.into());
    visiting.insert(id.into());
    for next in graph.0.get(id).into_iter().flatten() {
        if graph.1.contains_key(next) || graph.2.contains_key(next) || visiting.contains(next) {
            continue;
        }
        predecessors
            .entry(next.clone())
            .or_default()
            .push(id.into());
        if !roots.contains_key(next) {
            visit(next, root, graph, visiting, roots, predecessors);
        }
    }
    visiting.remove(id);
}
fn depth(id: &str, predecessors: &Edges, memo: &mut BTreeMap<String, usize>) -> usize {
    if let Some(d) = memo.get(id) {
        return *d;
    }
    let result = predecessors
        .get(id)
        .into_iter()
        .flatten()
        .map(|p| depth(p, predecessors, memo) + 1)
        .max()
        .unwrap_or(0);
    memo.insert(id.into(), result);
    result
}
fn place(
    roots: &[String],
    bases: &BTreeMap<String, usize>,
    edges: &Edges,
    owner: &BTreeMap<String, String>,
    placed: &mut BTreeMap<String, usize>,
    order: &mut Vec<String>,
    fallback: usize,
) {
    let mut visited = BTreeMap::new();
    let mut predecessors = BTreeMap::new();
    for root in roots {
        if !placed.contains_key(root) && !visited.contains_key(root) {
            visit(
                root,
                root,
                (edges, placed, owner),
                &mut BTreeSet::new(),
                &mut visited,
                &mut predecessors,
            );
        }
    }
    let mut memo = BTreeMap::new();
    for (id, root) in &visited {
        placed.insert(
            id.clone(),
            depth(id, &predecessors, &mut memo) + bases.get(root).copied().unwrap_or(fallback),
        );
    }
    let mut seen: BTreeSet<_> = order.iter().cloned().collect();
    let mut queue: VecDeque<_> = roots.iter().cloned().collect();
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id.clone()) || !placed.contains_key(&id) {
            continue;
        }
        order.push(id.clone());
        queue.extend(edges.get(&id).into_iter().flatten().cloned());
    }
}
