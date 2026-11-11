//! A mission as an ordered sequence (Archon's `flow/flowModel.ts`, ported
//! as it is): each Input card's steps numbered in the order a run meets
//! them, gates inline with their deciders and routes, judges nested under
//! their gate, and loops as references back to earlier steps. The walk
//! goes breadth first along outputs, passes before fails, never enters a
//! judge chain as a step, and numbers a node once, where it is first met.

use parvi_missions_contract::{Connection, Mission};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Step {
        node: String,
        number: Option<usize>,
        title: String,
        /// The route leads back to an earlier step (a loop).
        back: bool,
    },
    /// An End node: this path ends there, `done` or `rejected`.
    End {
        node: String,
        title: String,
        outcome: String,
    },
    /// A route not wired yet.
    Nowhere,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decider {
    You,
    Judge,
    Code,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepKind {
    Formation {
        next: Vec<Target>,
    },
    Gate {
        deciders: Vec<Decider>,
        /// The judge chain's formation ids.
        judges: Vec<String>,
        pass: Vec<Target>,
        fail: Vec<Target>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub id: String,
    pub number: usize,
    /// Siblings one output dispatches together; they share the number.
    pub parallel: Vec<String>,
    pub kind: StepKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// The Input card; `None` for the steps no Input card reaches.
    pub input: Option<String>,
    pub start: Vec<Target>,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flow {
    pub sections: Vec<Section>,
    pub numbers: HashMap<String, usize>,
    /// Judge formation id to the gate it judges.
    pub judge_of: HashMap<String, String>,
}

fn node_of(endpoint: &str) -> &str {
    endpoint.split(':').next().unwrap_or("")
}

fn port_of(endpoint: &str) -> &str {
    endpoint.split_once(':').map(|(_, p)| p).unwrap_or("")
}

/// The gate's judge chain: the steps wired from its judge port, each to
/// the next, and back to that port. A chain that leaves the steps, loops
/// or never returns is no chain.
pub fn judge_chain(m: &Mission, gate: &str) -> Vec<String> {
    let socket = format!("{gate}:judge");
    let Some(send) = m.connections.iter().find(|c| c.from == socket) else {
        return vec![];
    };
    let is_step = |id: &str| m.formations.iter().any(|f| f.id == id);
    let mut chain: Vec<String> = vec![];
    let mut current = node_of(&send.to).to_string();
    while !current.is_empty() && !chain.contains(&current) {
        if !is_step(&current) {
            return vec![];
        }
        chain.push(current.clone());
        let out: Vec<&Connection> = m
            .connections
            .iter()
            .filter(|c| node_of(&c.from) == current)
            .collect();
        if out.iter().any(|c| c.to == socket) {
            return chain;
        }
        current = out
            .iter()
            .map(|c| node_of(&c.to))
            .find(|n| is_step(n))
            .unwrap_or("")
            .to_string();
    }
    vec![]
}

pub fn build(m: &Mission) -> Flow {
    let title_of = |id: &str| -> String {
        m.formations
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.title.clone())
            .or_else(|| m.gates.iter().find(|g| g.id == id).map(|g| g.title.clone()))
            .or_else(|| {
                m.input_cards
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| c.title.clone())
            })
            .unwrap_or_else(|| id.to_string())
    };
    let mut judge_of = HashMap::new();
    for g in &m.gates {
        for j in judge_chain(m, &g.id) {
            judge_of.entry(j).or_insert_with(|| g.id.clone());
        }
    }
    // Work routes: judge sends and returns belong to the gate.
    let work_routes = |id: &str| -> Vec<&Connection> {
        m.connections
            .iter()
            .filter(|c| {
                node_of(&c.from) == id && port_of(&c.from) != "judge" && port_of(&c.to) != "judge"
            })
            .collect()
    };
    let is_step = |id: &str| {
        !judge_of.contains_key(id)
            && (m.formations.iter().any(|f| f.id == id) || m.gates.iter().any(|g| g.id == id))
    };
    // One output dispatches its siblings together; they share a number.
    let mut by_output: Vec<(String, Vec<String>)> = vec![];
    for c in &m.connections {
        if port_of(&c.from) == "judge" || port_of(&c.to) == "judge" || !is_step(node_of(&c.to)) {
            continue;
        }
        let to = node_of(&c.to).to_string();
        match by_output.iter_mut().find(|(f, _)| *f == c.from) {
            Some((_, list)) if !list.contains(&to) => list.push(to),
            Some(_) => {}
            None => by_output.push((c.from.clone(), vec![to])),
        }
    }
    let mut siblings: HashMap<String, Vec<String>> = HashMap::new();
    for (_, list) in by_output.iter().filter(|(_, l)| l.len() > 1) {
        for id in list {
            siblings.insert(id.clone(), list.clone());
        }
    }
    let mut numbers: HashMap<String, usize> = HashMap::new();
    let mut next = 0;
    let mut walk = |starts: Vec<String>, numbers: &mut HashMap<String, usize>| -> Vec<String> {
        let mut reached = vec![];
        let mut queue: VecDeque<String> = starts.into();
        while let Some(id) = queue.pop_front() {
            if numbers.contains_key(&id) || !is_step(&id) {
                continue;
            }
            let shared = siblings
                .get(&id)
                .and_then(|s| s.iter().find_map(|x| numbers.get(x).copied()));
            let n = shared.unwrap_or_else(|| {
                next += 1;
                next
            });
            numbers.insert(id.clone(), n);
            reached.push(id.clone());
            let routes = work_routes(&id);
            let onward = routes
                .iter()
                .filter(|r| port_of(&r.from) != "fail")
                .chain(routes.iter().filter(|r| port_of(&r.from) == "fail"));
            for r in onward {
                queue.push_back(node_of(&r.to).to_string());
            }
        }
        reached
    };
    let mut order: Vec<Vec<String>> = vec![];
    for card in &m.input_cards {
        let starts = work_routes(&card.id)
            .iter()
            .map(|r| node_of(&r.to).to_string())
            .collect();
        order.push(walk(starts, &mut numbers));
    }
    let unreached: Vec<String> = m
        .formations
        .iter()
        .map(|f| f.id.clone())
        .chain(m.gates.iter().map(|g| g.id.clone()))
        .filter(|id| is_step(id) && !numbers.contains_key(id))
        .collect();
    let rest = walk(unreached, &mut numbers);
    let targets = |from: usize, routes: Vec<&Connection>| -> Vec<Target> {
        routes
            .into_iter()
            .map(|r| {
                let node = node_of(&r.to).to_string();
                if let Some(end) = m.ends.iter().find(|e| e.id == node) {
                    return Target::End {
                        node,
                        title: end.title.clone(),
                        outcome: end.outcome.clone(),
                    };
                }
                let number = numbers.get(&node).copied();
                Target::Step {
                    title: title_of(&node),
                    back: number.is_some_and(|n| n <= from),
                    node,
                    number,
                }
            })
            .collect()
    };
    let or_nowhere = |t: Vec<Target>| {
        if t.is_empty() {
            vec![Target::Nowhere]
        } else {
            t
        }
    };
    let step_of = |id: &String| -> Step {
        let number = numbers[id];
        let routes = work_routes(id);
        let kind = match m.gates.iter().find(|g| &g.id == id) {
            Some(g) => {
                let on = |port: &str| {
                    or_nowhere(targets(
                        number,
                        routes
                            .iter()
                            .filter(|r| port_of(&r.from) == port)
                            .copied()
                            .collect(),
                    ))
                };
                StepKind::Gate {
                    deciders: [
                        ("human", Decider::You),
                        ("formation", Decider::Judge),
                        ("code", Decider::Code),
                    ]
                    .into_iter()
                    .filter(|(k, _)| g.kinds.iter().any(|x| x == k))
                    .map(|(_, d)| d)
                    .collect(),
                    judges: judge_chain(m, id),
                    pass: on("pass"),
                    fail: on("fail"),
                }
            }
            None => StepKind::Formation {
                next: or_nowhere(targets(number, routes)),
            },
        };
        Step {
            id: id.clone(),
            number,
            parallel: siblings.get(id).cloned().unwrap_or_default(),
            kind,
        }
    };
    let mut sections: Vec<Section> = m
        .input_cards
        .iter()
        .zip(&order)
        .map(|(card, ids)| Section {
            input: Some(card.id.clone()),
            start: targets(0, work_routes(&card.id)),
            steps: ids.iter().map(step_of).collect(),
        })
        .collect();
    if !rest.is_empty() {
        sections.push(Section {
            input: None,
            start: vec![],
            steps: rest.iter().map(step_of).collect(),
        });
    }
    Flow {
        sections,
        numbers,
        judge_of,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Archon's delivery mission, as the node serves it.
    pub fn delivery() -> Mission {
        let json = include_str!("testdata/delivery.json");
        serde_json::from_str::<parvi_missions_contract::MissionResponse>(json)
            .unwrap()
            .mission
            .mission
    }

    #[test]
    fn delivery_reads_in_run_order_with_its_judge_nested() {
        let flow = build(&delivery());
        let ids: Vec<(&str, usize)> = flow.sections[0]
            .steps
            .iter()
            .map(|s| (s.id.as_str(), s.number))
            .collect();
        assert_eq!(
            ids,
            [
                ("fmn_plan", 1),
                ("fmn_beads", 2),
                ("gate_beads_review", 3),
                ("fmn_execution", 4),
                ("fmn_final_review", 5)
            ]
        );
        assert_eq!(flow.judge_of["fmn_beads_reviewer"], "gate_beads_review");
        let StepKind::Gate {
            deciders,
            judges,
            fail,
            ..
        } = &flow.sections[0].steps[2].kind
        else {
            panic!("a gate")
        };
        assert_eq!(deciders, &[Decider::Judge]);
        assert_eq!(judges, &["fmn_beads_reviewer"]);
        assert!(matches!(
            &fail[0],
            Target::Step {
                back: true,
                number: Some(2),
                ..
            }
        ));
        let StepKind::Formation { next } = &flow.sections[0].steps[4].kind else {
            panic!("a formation")
        };
        assert!(matches!(&next[0], Target::End { outcome, .. } if outcome == "done"));
    }
}
