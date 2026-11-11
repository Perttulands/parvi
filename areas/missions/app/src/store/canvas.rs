//! A mission on its canvas (Archon's board): where each card sits, how big
//! it is, where its ports are, and the wires between them, in canvas units
//! before zoom. A mission with no saved layout reads left to right in run
//! order until it is arranged.

use super::flow::Flow;
use parvi_missions_contract::{LayoutNode, Mission};
use std::collections::HashMap;

/// Archon's card sizes (formations-d7.css). A step card is its IN rows,
/// its head, its slots and its OUT rows; ports sit on the rows.
pub const STEP_W: f32 = 320.;
/// A peer or orchestrated step is a little wider.
pub const WIDE_W: f32 = 336.;
pub const INPUT_W: f32 = 260.;
pub const GATE_W: f32 = 300.;
pub const GATE_H: f32 = 84.;
pub const END_W: f32 = 200.;
pub const END_H: f32 = 56.;
/// An IN or OUT row.
pub const IO_ROW: f32 = 34.;
/// A step's head: number, title, two lines of brief; a run adds its chips.
pub const STEP_HEAD: f32 = 92.;
/// Without a brief the head only needs room for the title and passport.
pub const STEP_HEAD_EMPTY: f32 = 78.;
pub const RUN_CHIPS: f32 = 30.;
/// The slots' body: its padding, each slot and the gap between them.
pub const BODY_PAD: f32 = 18.;
pub const SLOT_H: f32 = 58.;
pub const SLOT_GAP: f32 = 8.;
/// The dashed box holding an orchestrated step's workers or a peer group.
pub const POOL_PAD: f32 = 24.;
/// An Input card: eyebrow and title, goal, then one line for each fact.
pub const INPUT_HEAD: f32 = 74.;
pub const INPUT_GOAL: f32 = 40.;
pub const INPUT_LINE: f32 = 22.;
pub const INPUT_FOOT: f32 = 14.;
/// A port's dot, and how near a drop must land to take it.
pub const PORT: f32 = 18.;
pub const PORT_REACH: f32 = 16.;
const GAP_X: f32 = 80.;
const GAP_Y: f32 = 40.;
const MARGIN: f32 = 40.;
/// How far above the board a wire back runs, and how far out of its ports.
const LOOP: f32 = 24.;
pub const LOOP_OUT: f32 = 24.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Input,
    Formation,
    Gate,
    End,
}

/// One card on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: String,
    pub kind: Kind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A wire from one port to another, and what it routes.
#[derive(Clone, Debug, PartialEq)]
pub struct Wire {
    pub id: String,
    pub from: (f32, f32),
    pub to: (f32, f32),
    /// `pass`, `fail`, `judge`, or blank for work.
    pub route: String,
    pub from_node: String,
    pub to_node: String,
    /// A wire back to an earlier card runs above the board at this height
    /// instead of curving behind the cards between (Archon's loop).
    pub lane: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    pub cards: Vec<Card>,
    pub wires: Vec<Wire>,
}

/// Whether a step's workers or peers sit in a pool under its controller.
pub fn pooled(kind: &str) -> bool {
    matches!(kind, "orchestrated" | "peer")
}

/// The height of the slots' body for `slots` slots.
pub fn body_h(kind: &str, slots: usize) -> f32 {
    let n = slots.max(1) as f32;
    BODY_PAD * 2. + n * SLOT_H + (n - 1.) * SLOT_GAP + if pooled(kind) { POOL_PAD } else { 0. }
}

/// The face and the board must agree on the head height so ports meet rows.
pub fn step_head(f: &parvi_missions_contract::Formation, running: bool) -> f32 {
    let has_brief = f.brief.as_ref().is_some_and(|b| !b.goal.trim().is_empty());
    (if has_brief {
        STEP_HEAD
    } else {
        STEP_HEAD_EMPTY
    }) + if running { RUN_CHIPS } else { 0. }
}

fn input_lines(c: &parvi_missions_contract::InputCard) -> f32 {
    // Inputs, and the human-gate channel.
    1. + f32::from(u8::from(!c.inputs.is_empty()))
}

fn size(m: &Mission, id: &str, running: bool) -> Option<(Kind, f32, f32)> {
    if let Some(c) = m.input_cards.iter().find(|c| c.id == id) {
        let h = INPUT_HEAD + INPUT_GOAL + INPUT_LINE * input_lines(c) + INPUT_FOOT;
        return Some((Kind::Input, INPUT_W, h));
    }
    if let Some(f) = m.formations.iter().find(|f| f.id == id) {
        let w = if pooled(&f.kind) { WIDE_W } else { STEP_W };
        let rows = (f.inputs.len() + f.outputs.len()) as f32;
        let head = step_head(f, running);
        return Some((
            Kind::Formation,
            w,
            rows * IO_ROW + head + body_h(&f.kind, f.slots.len()),
        ));
    }
    if m.gates.iter().any(|g| g.id == id) {
        return Some((Kind::Gate, GATE_W, GATE_H));
    }
    if m.ends.iter().any(|e| e.id == id) {
        return Some((Kind::End, END_W, END_H));
    }
    None
}

/// Run order, left to right: Input cards first, each numbered step in its
/// column (siblings stacked), a gate's judges under it, Ends last.
pub fn arranged(m: &Mission, flow: &Flow) -> Vec<LayoutNode> {
    let mut columns: Vec<Vec<String>> = vec![m.input_cards.iter().map(|c| c.id.clone()).collect()];
    let mut by_number: Vec<(usize, String)> = flow
        .numbers
        .iter()
        .map(|(id, n)| (*n, id.clone()))
        .collect();
    by_number.sort();
    for (n, id) in by_number {
        while columns.len() <= n {
            columns.push(vec![]);
        }
        columns[n].push(id.clone());
        // A gate's judges sit under it.
        for (judge, gate) in &flow.judge_of {
            if *gate == id {
                columns[n].push(judge.clone());
            }
        }
    }
    columns.push(m.ends.iter().map(|e| e.id.clone()).collect());
    let mut out = vec![];
    let mut x = MARGIN;
    for col in columns.iter().filter(|c| !c.is_empty()) {
        let mut y = MARGIN;
        let mut w: f32 = 0.;
        for id in col {
            let Some((_, cw, ch)) = size(m, id, false) else {
                continue;
            };
            out.push(LayoutNode {
                id: id.clone(),
                x: x as i64,
                y: y as i64,
            });
            y += ch + GAP_Y;
            w = w.max(cw);
        }
        x += w + GAP_X;
    }
    out
}

fn port_of(endpoint: &str) -> (&str, &str) {
    endpoint.split_once(':').unwrap_or((endpoint, ""))
}

/// Where a port is: inputs on the left on their IN rows, outputs on the
/// right on their OUT rows, a gate's pass and fail on its right and its
/// judge port under it.
fn port_at(m: &Mission, c: &Card, port: &str, out: bool) -> (f32, f32) {
    let side = if out { c.x + c.w } else { c.x };
    match c.kind {
        Kind::Gate => match port {
            "pass" => (side, c.y + c.h / 3.),
            "fail" => (side, c.y + c.h * 2. / 3.),
            "judge" => (c.x + c.w / 2., c.y + c.h),
            _ => (side, c.y + c.h / 2.),
        },
        Kind::Formation => {
            let f = m.formations.iter().find(|f| f.id == c.id);
            let ports = f.map(|f| if out { &f.outputs } else { &f.inputs });
            let n = ports.map(|p| p.len()).unwrap_or(0);
            let i = ports
                .and_then(|p| p.iter().position(|p| p.id == port))
                .unwrap_or(0);
            let row = if out {
                c.h - IO_ROW * (n - i) as f32
            } else {
                IO_ROW * i as f32
            };
            (side, c.y + row + IO_ROW / 2.)
        }
        _ => (side, c.y + c.h / 2.),
    }
}

/// A port on the board: its endpoint (`node:port`), where it is, whether
/// wires leave it, and whether a wire is on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub endpoint: String,
    pub at: (f32, f32),
    pub out: bool,
    pub wired: bool,
}

/// Every port on the board. A gate's judge port is both: its question
/// leaves it and the verdict returns to it.
pub fn ports(m: &Mission, b: &Board) -> Vec<Port> {
    let mut out = vec![];
    let wired = |e: &str| m.connections.iter().any(|c| c.from == e || c.to == e);
    let mut add = |c: &Card, port: &str, leaves: bool| {
        let endpoint = format!("{}:{port}", c.id);
        out.push(Port {
            at: port_at(m, c, port, leaves),
            out: leaves,
            wired: wired(&endpoint),
            endpoint,
        });
    };
    for c in &b.cards {
        match c.kind {
            Kind::Input => add(c, "out", true),
            Kind::Formation => {
                if let Some(f) = m.formations.iter().find(|f| f.id == c.id) {
                    for p in &f.inputs {
                        add(c, &p.id, false);
                    }
                    for p in &f.outputs {
                        add(c, &p.id, true);
                    }
                }
            }
            Kind::Gate => {
                add(c, "in", false);
                add(c, "pass", true);
                add(c, "fail", true);
                add(c, "judge", true);
            }
            Kind::End => add(c, "in", false),
        }
    }
    out
}

/// The port a drop at `at` lands on: the nearest within reach that takes
/// a wire from `from` (an input for an output; the judge port takes
/// either way), never on the card the wire leaves.
pub fn drop_target<'a>(ports: &'a [Port], from: &str, at: (f32, f32)) -> Option<&'a Port> {
    let node = from.split(':').next().unwrap_or("");
    ports
        .iter()
        .filter(|p| {
            (!p.out || p.endpoint.ends_with(":judge")) && p.endpoint.split(':').next() != Some(node)
        })
        .map(|p| (p, (p.at.0 - at.0).hypot(p.at.1 - at.1)))
        .filter(|(_, d)| *d <= PORT_REACH)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| p)
}

/// The board for a mission at these places; a card the layout misses is
/// placed where the run-order arrangement puts it.
/// `running` gives step cards room for their run chips.
pub fn board(m: &Mission, flow: &Flow, layout: &[LayoutNode], running: bool) -> Board {
    let fallback = arranged(m, flow);
    let mut at: HashMap<&str, (f32, f32)> = fallback
        .iter()
        .map(|p| (p.id.as_str(), (p.x as f32, p.y as f32)))
        .collect();
    for p in layout {
        at.insert(p.id.as_str(), (p.x as f32, p.y as f32));
    }
    let ids = m
        .input_cards
        .iter()
        .map(|c| &c.id)
        .chain(m.formations.iter().map(|f| &f.id))
        .chain(m.gates.iter().map(|g| &g.id))
        .chain(m.ends.iter().map(|e| &e.id));
    let cards: Vec<Card> = ids
        .filter_map(|id| {
            let (kind, w, h) = size(m, id, running)?;
            let (x, y) = at.get(id.as_str()).copied().unwrap_or((MARGIN, MARGIN));
            Some(Card {
                id: id.clone(),
                kind,
                x,
                y,
                w,
                h,
            })
        })
        .collect();
    let card = |id: &str| cards.iter().find(|c| c.id == id);
    let top = cards.iter().map(|c| c.y).fold(f32::MAX, f32::min);
    let wires = m
        .connections
        .iter()
        .filter_map(|c| {
            let (from_node, from_port) = port_of(&c.from);
            let (to_node, to_port) = port_of(&c.to);
            let (a, b) = (card(from_node)?, card(to_node)?);
            // A judge's verdict returns to the gate's judge port from below.
            let returning = to_port == "judge";
            let route = match (from_port, returning) {
                ("pass" | "fail" | "judge", _) => from_port.to_string(),
                (_, true) => "judge".into(),
                _ => String::new(),
            };
            let from = port_at(m, a, from_port, true);
            let to = if returning {
                (b.x + b.w / 2., b.y + b.h)
            } else {
                port_at(m, b, to_port, false)
            };
            let back = route != "judge" && to.0 < from.0;
            Some(Wire {
                id: c.id.clone(),
                from,
                to,
                lane: back.then_some(top - LOOP),
                route,
                from_node: from_node.to_string(),
                to_node: to_node.to_string(),
            })
        })
        .collect();
    Board { cards, wires }
}

/// The smallest rectangle holding every card: x, y, width, height.
pub fn extent(b: &Board) -> (f32, f32, f32, f32) {
    if b.cards.is_empty() {
        return (0., 0., 1., 1.);
    }
    let x0 = b.cards.iter().map(|c| c.x).fold(f32::MAX, f32::min);
    let y0 = b.cards.iter().map(|c| c.y).fold(f32::MAX, f32::min);
    let x1 = b.cards.iter().map(|c| c.x + c.w).fold(f32::MIN, f32::max);
    let y1 = b.cards.iter().map(|c| c.y + c.h).fold(f32::MIN, f32::max);
    (x0, y0, x1 - x0, y1 - y0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::flow::{build, tests::delivery};

    #[test]
    fn an_unarranged_mission_reads_left_to_right_in_run_order() {
        let m = delivery();
        let b = board(&m, &build(&m), &[], false);
        let x = |id: &str| b.cards.iter().find(|c| c.id == id).unwrap().x;
        assert!(x("mis_delivery") < x("fmn_plan"));
        assert!(x("fmn_plan") < x("fmn_beads"));
        assert!(x("fmn_execution") < x("fmn_final_review"));
        assert!(x("fmn_final_review") < x("end_delivered"));
        // The judge sits under its gate.
        let gate = b
            .cards
            .iter()
            .find(|c| c.id == "gate_beads_review")
            .unwrap();
        let judge = b
            .cards
            .iter()
            .find(|c| c.id == "fmn_beads_reviewer")
            .unwrap();
        assert_eq!(gate.x, judge.x);
        assert!(judge.y > gate.y + gate.h);
        assert_eq!(b.wires.len(), m.connections.len());
        let fail = b.wires.iter().find(|w| w.route == "fail").unwrap();
        assert_eq!(fail.to_node, "fmn_beads");
        // Sent back, it loops above every card rather than behind them.
        let top = b.cards.iter().map(|c| c.y).fold(f32::MAX, f32::min);
        assert!(fail.lane.is_some_and(|y| y < top));
        let pass = b.wires.iter().find(|w| w.route == "pass").unwrap();
        assert_eq!(pass.lane, None);
    }

    #[test]
    fn a_saved_place_wins_over_the_arrangement() {
        let m = delivery();
        let saved = [LayoutNode {
            id: "fmn_plan".into(),
            x: 900,
            y: 700,
        }];
        let b = board(&m, &build(&m), &saved, false);
        let plan = b.cards.iter().find(|c| c.id == "fmn_plan").unwrap();
        assert_eq!((plan.x, plan.y), (900., 700.));
        let into = b.wires.iter().find(|w| w.to_node == "fmn_plan").unwrap();
        assert_eq!(into.to.0, 900.);
    }

    #[test]
    fn empty_briefs_leave_no_band_and_outputs_follow_the_card() {
        let mut m = delivery();
        let id = m.formations[0].id.clone();
        m.formations[0].brief = Some(parvi_missions_contract::Brief {
            goal: "A brief".into(),
            ..Default::default()
        });
        let full = board(&m, &build(&m), &[], false);
        m.formations[0].brief.as_mut().unwrap().goal = " \n ".into();
        let compact = board(&m, &build(&m), &[], false);
        let card = |b: &Board| b.cards.iter().find(|c| c.id == id).unwrap().clone();
        assert_eq!(
            card(&full).h - card(&compact).h,
            STEP_HEAD - STEP_HEAD_EMPTY
        );
        let full_ports = ports(&m, &full);
        let compact_ports = ports(&m, &compact);
        let output = compact_ports
            .iter()
            .find(|p| p.out && p.endpoint.starts_with(&format!("{id}:")))
            .unwrap();
        let old = full_ports
            .iter()
            .find(|p| p.endpoint == output.endpoint)
            .unwrap();
        assert_eq!(old.at.1 - output.at.1, STEP_HEAD - STEP_HEAD_EMPTY);
        assert_eq!(
            step_head(&m.formations[0], true),
            STEP_HEAD_EMPTY + RUN_CHIPS
        );
    }

    #[test]
    fn a_wire_drops_on_the_input_port_nearest_the_pointer() {
        let m = delivery();
        let b = board(&m, &build(&m), &[], false);
        let ports = ports(&m, &b);
        let into_beads = ports
            .iter()
            .find(|p| p.endpoint == "fmn_beads:port_beads_in")
            .unwrap();
        assert!(!into_beads.out && into_beads.wired);
        let beads = b.cards.iter().find(|c| c.id == "fmn_beads").unwrap();
        // On the card's left edge, on its IN row.
        assert_eq!(into_beads.at, (beads.x, beads.y + IO_ROW / 2.));
        let near = (into_beads.at.0 + 5., into_beads.at.1 - 5.);
        let hit = drop_target(&ports, "fmn_plan:port_plan_out", near).unwrap();
        assert_eq!(hit.endpoint, "fmn_beads:port_beads_in");
        // Not onto the card the wire leaves, nor onto an output.
        assert!(drop_target(&ports, "fmn_beads:port_beads_out", near).is_none());
        let out = ports
            .iter()
            .find(|p| p.endpoint == "fmn_plan:port_plan_out")
            .unwrap();
        assert!(drop_target(&ports, "mis_delivery:out", out.at).is_none());
        // A gate's judge port takes the judge's verdict.
        let judge = ports
            .iter()
            .find(|p| p.endpoint == "gate_beads_review:judge")
            .unwrap();
        let hit = drop_target(
            &ports,
            "fmn_beads_reviewer:port_beads_reviewer_out",
            judge.at,
        );
        assert_eq!(
            hit.map(|p| p.endpoint.as_str()),
            Some("gate_beads_review:judge")
        );
    }
}
