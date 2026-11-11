//! Undo and redo of mission edits, as Archon's cockpit keeps them
//! (formationsUndo.ts, formationsRedo.ts). An edit is one or more steps,
//! each a mission patch or a layout patch. Before a step is written, the
//! step that reverses it is worked out from the mission as it is then; an
//! entry keeps those reverses, newest first, so undo never needs the
//! mission as it is later. Undoing an entry writes its steps and keeps
//! their own reverses as the redo.

use parvi_missions_contract::LayoutNode as Placed;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// A mission patch: `{"<operation>": {...}}`.
    Board(Value),
    /// A layout patch: where cards sit.
    Layout(Vec<Placed>),
}

/// One edit's undo: its steps run in order. `label` names the edit, as in
/// "Undid the move of “Plan”".
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub label: String,
    pub steps: Vec<Step>,
}

/// What a mission was as of a write: its revision and its layout's ETag
/// (none before it has a layout).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Version {
    pub rev: i64,
    pub layout: Option<String>,
}

/// Undo or redo refused: the mission is no longer as this history's last
/// write left it, so writing the entry would overwrite someone else's change.
#[derive(Debug, PartialEq, Eq)]
pub struct Stale;

/// One mission's history.
#[derive(Default, Debug)]
pub struct History {
    undos: Vec<Entry>,
    redos: Vec<Entry>,
    /// The version this history's last write left.
    at: Option<Version>,
}

impl History {
    /// This history's last write left the mission at `v`.
    pub fn wrote(&mut self, v: Version) {
        self.at = Some(v);
    }

    /// The entry to undo (or redo) if the mission is still `now`, as this
    /// history's last write left it. Otherwise another client has written
    /// since: the entries were worked out against what it overwrote, so the
    /// history is dropped and the step refused.
    pub fn take(&mut self, redo: bool, now: &Version) -> Result<Option<Entry>, Stale> {
        let stack = if redo { &self.redos } else { &self.undos };
        if stack.is_empty() {
            return Ok(None);
        }
        if self.at.as_ref() != Some(now) {
            *self = History::default();
            return Err(Stale);
        }
        Ok(if redo {
            self.redos.pop()
        } else {
            self.undos.pop()
        })
    }

    /// A new edit: it can be undone, and what was undone can no longer be
    /// redone.
    pub fn record(&mut self, e: Entry) {
        if !e.steps.is_empty() {
            self.undos.push(e);
            self.redos.clear();
        }
    }

    /// What undoing wrote, reversed: the redo.
    pub fn undone(&mut self, e: Entry) {
        if !e.steps.is_empty() {
            self.redos.push(e);
        }
    }

    /// What redoing wrote, reversed: the undo again.
    pub fn redone(&mut self, e: Entry) {
        if !e.steps.is_empty() {
            self.undos.push(e);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undos.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redos.is_empty()
    }
}

const KINDS: [(&str, &str, &str); 5] = [
    ("inputCard", "inputCards", "Input card"),
    ("formation", "formations", "formation"),
    ("gate", "gates", "gate"),
    ("end", "ends", "End node"),
    ("limit", "limits", "Limit card"),
];

fn list<'a>(m: &'a Value, key: &str) -> &'a [Value] {
    m[key].as_array().map(Vec::as_slice).unwrap_or_default()
}

/// A node of the mission: its kind, itself as the node sent it, and its
/// place among the nodes of its kind.
fn node_of<'a>(m: &'a Value, id: &str) -> Option<(&'static str, &'static str, &'a Value, usize)> {
    KINDS.iter().find_map(|(kind, key, word)| {
        list(m, key)
            .iter()
            .position(|n| n["id"] == id)
            .map(|i| (*kind, *word, &list(m, key)[i], i))
    })
}

pub fn quoted(title: &str, fallback: &str) -> String {
    if title.is_empty() {
        fallback.to_string()
    } else {
        format!("“{title}”")
    }
}

/// How an edit of a node is named: `formation “Plan”`.
pub fn node_words(m: &Value, id: &str) -> String {
    match node_of(m, id) {
        Some((_, word, n, _)) => format!(
            "{word} {}",
            quoted(n["title"].as_str().unwrap_or(""), "untitled")
        ),
        None => "a card".into(),
    }
}

/// The step that puts back a deleted node: the node as the node sent it,
/// every wire touching it, where it sat and its place among its kind.
pub fn restore_step(m: &Value, id: &str, at: (i64, i64)) -> Option<Step> {
    let (kind, _, node, index) = node_of(m, id)?;
    let wires: Vec<Value> = list(m, "connections")
        .iter()
        .filter(|c| {
            let end = |k: &str| c[k].as_str().unwrap_or("").split(':').next() == Some(id);
            end("from") || end("to")
        })
        .map(|c| json!({"id": c["id"], "from": c["from"], "to": c["to"]}))
        .collect();
    Some(Step::Board(json!({"restoreNode": {
        kind: node, "connections": wires, "index": index,
        "x": at.0, "y": at.1,
    }})))
}

fn op_of(patch: &Value) -> Option<(&str, &Value)> {
    patch
        .as_object()?
        .iter()
        .next()
        .map(|(k, v)| (k.as_str(), v))
}

/// The steps that reverse `step` when written to mission `m` with layout
/// `layout`, worked out before it is written. A create is reversed once
/// its answer names what it made (`created`).
pub fn inverse(m: &Value, layout: &[Placed], step: &Step) -> Result<Vec<Step>, String> {
    let patch = match step {
        Step::Layout(nodes) => {
            return Ok(vec![Step::Layout(
                nodes
                    .iter()
                    .filter_map(|n| layout.iter().find(|p| p.id == n.id).cloned())
                    .collect(),
            )]);
        }
        Step::Board(p) => p,
    };
    let (op, v) = op_of(patch).ok_or("an empty edit")?;
    let id = v["id"].as_str().unwrap_or("");
    let board = |p: Value| Ok(vec![Step::Board(p)]);
    if op == "title" {
        return board(json!({"title": m["title"]}));
    }
    if op.starts_with("create") || op == "removePort" {
        // Reversed from its answer: the id it made, or the wire a removed
        // port took with it, is known once it is written.
        return Ok(vec![]);
    }
    if op.starts_with("delete") {
        let at = layout
            .iter()
            .find(|p| p.id == id)
            .map(|p| (p.x, p.y))
            .unwrap_or((200, 200));
        return restore_step(m, id, at)
            .map(|s| vec![s])
            .ok_or_else(|| "the card is already gone".to_string());
    }
    if op == "restoreNode" {
        let (kind, node) = KINDS
            .iter()
            .find_map(|(k, _, _)| v.get(*k).filter(|n| !n.is_null()).map(|n| (*k, n)))
            .ok_or("the restored card has no kind")?;
        let mut name = kind.to_string();
        name[..1].make_ascii_uppercase();
        return board(json!({ format!("delete{name}"): {"id": node["id"]} }));
    }
    if op == "updatePort" {
        let formation = v["formationId"].as_str().unwrap_or("");
        let slot = list(m, "formations")
            .iter()
            .find(|f| f["id"] == formation)
            .and_then(|f| {
                list(f, "inputs")
                    .iter()
                    .chain(list(f, "outputs"))
                    .find(|p| p["id"] == v["portId"])
            })
            .ok_or("the edited port no longer exists")?;
        let mut fields = json!({"formationId":formation,"portId":v["portId"]});
        for key in ["label", "kind"] {
            if v.get(key).is_some() {
                fields[key] = slot.get(key).cloned().unwrap_or_else(|| json!(""));
            }
        }
        return board(json!({"updatePort":fields}));
    }
    if op.starts_with("update") || op == "editFormation" {
        let (_, _, node, _) = node_of(m, id).ok_or("the edited card no longer exists")?;
        let mut fields = serde_json::Map::new();
        fields.insert("id".into(), id.into());
        for key in v.as_object().into_iter().flatten().map(|(k, _)| k) {
            if key == "id" {
                continue;
            }
            let before = match node.get(key) {
                Some(x) if !x.is_null() => x.clone(),
                _ => match key.as_str() {
                    "files" | "inputs" | "outputs" | "kinds" => json!([]),
                    "rounds" | "seconds" | "warnSeconds" | "tokens" => json!(0),
                    "brief" => json!({}),
                    _ => json!(""),
                },
            };
            fields.insert(key.clone(), before);
        }
        return board(json!({ op: fields }));
    }
    if op == "assignSlot" {
        let f = v["formationId"].as_str().unwrap_or("");
        let slot = list(m, "formations")
            .iter()
            .find(|n| n["id"] == f)
            .and_then(|n| list(n, "slots").iter().find(|s| s["id"] == v["slotId"]))
            .ok_or("the staffed slot no longer exists")?;
        let w = |k: &str| slot[k].as_str().unwrap_or("").to_string();
        return board(json!({"assignSlot": {
            "formationId": f, "slotId": v["slotId"], "agentId": w("agentId"),
            "harness": w("harness"), "model": w("model"), "effort": w("effort"),
        }}));
    }
    if op == "wireConnection" && v["joinIfOccupied"] == true {
        // A join may add a port: reversed from its answer.
        return Ok(vec![]);
    }
    if op == "wireConnection" {
        return board(json!({"unwireConnection": {"from": v["from"], "to": v["to"]}}));
    }
    if op == "unwireConnection" {
        let wire = list(m, "connections")
            .iter()
            .find(|c| c["from"] == v["from"] && c["to"] == v["to"])
            .ok_or("the wire no longer exists")?;
        return board(
            json!({"wireConnection": {"id": wire["id"], "from": wire["from"], "to": wire["to"]}}),
        );
    }
    if op == "rewireConnection" && v["joinIfOccupied"] == true {
        return Ok(vec![]);
    }
    if op == "rewireConnection" {
        return board(json!({"rewireConnection": {
            "from": v["from"], "previousTo": v["to"], "to": v["previousTo"],
        }}));
    }
    Err(format!("{op} cannot be undone"))
}

/// The reverse of a step only its answer can tell (Archon's wire and
/// rewireTarget): what a create made, or where a wire that may have
/// joined an occupied input actually landed.
pub fn answered(patch: &Value, before: &Value, after: &Value) -> Vec<Step> {
    let Some((op, v)) = op_of(patch) else {
        return vec![];
    };
    let wires = |m: &'_ Value| list(m, "connections").to_vec();
    let landed = |id: &Value| {
        wires(after)
            .into_iter()
            .find(|c| c["id"] == *id)
            .map(|c| c["to"].clone())
    };
    match op {
        "wireConnection" => {
            let added = wires(after).into_iter().find(|c| {
                c["from"] == v["from"] && !wires(before).iter().any(|b| b["id"] == c["id"])
            });
            let Some(added) = added else { return vec![] };
            if added["to"] == v["to"] {
                return vec![Step::Board(
                    json!({"unwireConnection": {"from": v["from"], "to": v["to"]}}),
                )];
            }
            // It joined: the port it added goes again, with its wire.
            let to = added["to"].as_str().unwrap_or("");
            let (formation, port) = to.split_once(':').unwrap_or((to, ""));
            vec![Step::Board(
                json!({"removePort": {"formationId": formation, "portId": port}}),
            )]
        }
        "rewireConnection" => {
            let old = wires(before)
                .into_iter()
                .find(|c| c["from"] == v["from"] && c["to"] == v["previousTo"]);
            let Some(old) = old else { return vec![] };
            let Some(to) = landed(&old["id"]) else {
                return vec![];
            };
            let mut back = json!({"from": v["from"], "previousTo": to, "to": v["previousTo"]});
            if to != v["to"] {
                back["removePreviousInput"] = json!(true);
            }
            vec![Step::Board(json!({"rewireConnection": back}))]
        }
        // An undone join is redone by joining again: the wire the port
        // took goes back to an input of that step, which joins it anew.
        "removePort" => {
            let formation = v["formationId"].as_str().unwrap_or("");
            let port = format!("{formation}:{}", v["portId"].as_str().unwrap_or(""));
            let mine = |c: &Value| {
                c["to"]
                    .as_str()
                    .is_some_and(|t| t.split_once(':').is_some_and(|(f, _)| f == formation))
            };
            let first_input = || {
                list(after, "formations")
                    .iter()
                    .find(|f| f["id"] == formation)
                    .and_then(|f| f["inputs"][0]["id"].as_str())
                    .map(|p| json!(format!("{formation}:{p}")))
            };
            let Some(to) = wires(after)
                .iter()
                .find(|c| mine(c))
                .map(|c| c["to"].clone())
                .or_else(first_input)
            else {
                return vec![];
            };
            wires(before)
                .iter()
                .filter(|c| c["to"] == json!(port))
                .map(|c| {
                    Step::Board(json!({"wireConnection": {
                        "from": c["from"], "to": to, "joinIfOccupied": true,
                    }}))
                })
                .collect()
        }
        _ => created(before, after)
            .map(|(id, kind)| vec![uncreate(&id, kind)])
            .unwrap_or_default(),
    }
}

/// The id a create made: in the mission after it, not before.
pub fn created(before: &Value, after: &Value) -> Option<(String, &'static str)> {
    KINDS.iter().find_map(|(kind, key, _)| {
        list(after, key)
            .iter()
            .filter_map(|n| n["id"].as_str())
            .find(|id| !list(before, key).iter().any(|b| b["id"] == *id))
            .map(|id| (id.to_string(), *kind))
    })
}

/// The reverse of a create, from what it made.
pub fn uncreate(id: &str, kind: &str) -> Step {
    let mut name = kind.to_string();
    name[..1].make_ascii_uppercase();
    Step::Board(json!({ format!("delete{name}"): {"id": id} }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mission() -> Value {
        json!({
            "title": "Delivery",
            "formations": [
                {"id": "fmn_plan", "title": "Plan", "type": "solo",
                 "slots": [{"id": "s1", "agentId": "planner", "harness": "claude-code", "model": "", "effort": "xhigh"}]},
                {"id": "fmn_beads", "title": "Beads", "type": "solo", "slots": []}
            ],
            "gates": [{"id": "gate_r", "title": "Review", "kinds": ["formation"], "criterion": "ok"}],
            "connections": [
                {"id": "c1", "from": "fmn_plan:out", "to": "fmn_beads:in"},
                {"id": "c2", "from": "fmn_beads:out", "to": "gate_r:in"}
            ]
        })
    }

    #[test]
    fn a_delete_is_undone_by_restoring_the_card_with_its_wires_where_it_sat() {
        let m = mission();
        let at = [Placed {
            id: "fmn_beads".into(),
            x: 420,
            y: 80,
        }];
        let del = Step::Board(json!({"deleteFormation": {"id": "fmn_beads"}}));
        let inv = inverse(&m, &at, &del).unwrap();
        let Step::Board(p) = &inv[0] else { panic!() };
        let r = &p["restoreNode"];
        assert_eq!(r["formation"]["title"], "Beads");
        assert_eq!(r["connections"].as_array().unwrap().len(), 2);
        assert_eq!((r["x"].as_i64(), r["index"].as_u64()), (Some(420), Some(1)));
        // And the restore is undone by deleting it again: the redo.
        let back = inverse(&m, &at, &inv[0]).unwrap();
        assert_eq!(back, vec![del]);
    }

    #[test]
    fn an_update_is_undone_by_writing_back_only_the_fields_it_changed() {
        let m = mission();
        let up = Step::Board(json!({"updateGate": {"id": "gate_r", "criterion": "better"}}));
        let inv = inverse(&m, &[], &up).unwrap();
        assert_eq!(
            inv,
            vec![Step::Board(
                json!({"updateGate": {"id": "gate_r", "criterion": "ok"}})
            )]
        );
        let staff = Step::Board(
            json!({"assignSlot": {"formationId": "fmn_plan", "slotId": "s1", "agentId": "", "harness": "openai-codex", "model": "", "effort": "high"}}),
        );
        let Step::Board(p) = &inverse(&m, &[], &staff).unwrap()[0] else {
            panic!()
        };
        assert_eq!(p["assignSlot"]["harness"], "claude-code");
        assert_eq!(p["assignSlot"]["agentId"], "planner");
    }

    #[test]
    fn wires_moves_and_creates_reverse() {
        let m = mission();
        let unwire = Step::Board(
            json!({"unwireConnection": {"from": "fmn_plan:out", "to": "fmn_beads:in"}}),
        );
        let Step::Board(p) = &inverse(&m, &[], &unwire).unwrap()[0] else {
            panic!()
        };
        assert_eq!(p["wireConnection"]["id"], "c1");
        let at = [Placed {
            id: "fmn_plan".into(),
            x: 40,
            y: 40,
        }];
        let mv = Step::Layout(vec![Placed {
            id: "fmn_plan".into(),
            x: 300,
            y: 90,
        }]);
        assert_eq!(
            inverse(&m, &at, &mv).unwrap(),
            vec![Step::Layout(at.to_vec())]
        );
        let mut after = m.clone();
        after["formations"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "fmn_new", "title": "New"}));
        assert_eq!(created(&m, &after), Some(("fmn_new".into(), "formation")));
        assert_eq!(
            uncreate("fmn_new", "formation"),
            Step::Board(json!({"deleteFormation": {"id": "fmn_new"}}))
        );
    }

    #[test]
    fn a_new_edit_clears_the_redo() {
        let mut h = History::default();
        let e = |l: &str| Entry {
            label: l.into(),
            steps: vec![Step::Layout(vec![])],
        };
        h.record(e("a"));
        h.wrote(Version::default());
        let a = h.take(false, &Version::default()).unwrap().unwrap();
        h.undone(a);
        assert!(h.can_redo());
        h.record(e("b"));
        assert!(!h.can_redo());
        assert!(h.can_undo());
    }

    #[test]
    fn undo_is_refused_once_another_client_has_written_the_mission() {
        // A renames a step (rev 4 -> 5); B renames it again (rev 6). A's
        // undo would write A's old title over B's: it is refused, and the
        // history, worked out against what B overwrote, is dropped.
        let mut h = History::default();
        let rename = Entry {
            label: "the title".into(),
            steps: vec![Step::Board(
                json!({"updateFormation": {"id": "fmn_plan", "title": "A"}}),
            )],
        };
        h.record(rename.clone());
        let mine = Version {
            rev: 5,
            layout: Some("l1".into()),
        };
        h.wrote(mine.clone());
        let theirs = Version {
            rev: 6,
            ..mine.clone()
        };
        assert_eq!(h.take(false, &theirs), Err(Stale));
        assert!(!h.can_undo() && !h.can_redo());
        // A layout written elsewhere refuses it the same way.
        h.record(rename.clone());
        h.wrote(mine.clone());
        let moved = Version {
            layout: Some("l2".into()),
            ..mine.clone()
        };
        assert_eq!(h.take(false, &moved), Err(Stale));
        // Untouched since, the undo goes ahead, and so does the redo after
        // the undo's own write.
        h.record(rename.clone());
        h.wrote(mine.clone());
        let e = h.take(false, &mine).unwrap().unwrap();
        h.undone(e);
        let undone = Version { rev: 6, ..mine };
        h.wrote(undone.clone());
        assert_eq!(h.take(true, &undone).unwrap(), Some(rename));
    }

    #[test]
    fn a_wire_that_joined_an_occupied_input_is_undone_by_removing_the_port_it_added() {
        let before = mission();
        let mut after = before.clone();
        after["connections"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "c3", "from": "gate_r:pass", "to": "fmn_beads:in_2"}));
        let wire = json!({"wireConnection": {"from": "gate_r:pass", "to": "fmn_beads:in", "joinIfOccupied": true}});
        assert_eq!(
            inverse(&before, &[], &Step::Board(wire.clone())).unwrap(),
            vec![]
        );
        assert_eq!(
            answered(&wire, &before, &after),
            vec![Step::Board(
                json!({"removePort": {"formationId": "fmn_beads", "portId": "in_2"}})
            )]
        );
        after["connections"].as_array_mut().unwrap().pop();
        after["connections"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "c3", "from": "gate_r:pass", "to": "fmn_plan:in"}));
        let plain = json!({"wireConnection": {"from": "gate_r:pass", "to": "fmn_plan:in", "joinIfOccupied": true}});
        let Step::Board(p) = &answered(&plain, &before, &after)[0] else {
            panic!()
        };
        assert_eq!(p["unwireConnection"]["to"], "fmn_plan:in");
    }

    #[test]
    fn an_undone_join_is_redone_by_joining_the_same_input_again() {
        let joined = {
            let mut m = mission();
            m["connections"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id": "c3", "from": "gate_r:pass", "to": "fmn_beads:in_2"}));
            m
        };
        let remove = json!({"removePort": {"formationId": "fmn_beads", "portId": "in_2"}});
        assert_eq!(
            inverse(&joined, &[], &Step::Board(remove.clone())).unwrap(),
            vec![]
        );
        assert_eq!(
            answered(&remove, &joined, &mission()),
            vec![Step::Board(json!({"wireConnection": {
                "from": "gate_r:pass", "to": "fmn_beads:in", "joinIfOccupied": true,
            }}))]
        );
    }
    #[test]
    fn a_port_edit_reverses_only_its_changed_fields() {
        let mut m = mission();
        m["formations"][0]["inputs"] = json!([{"id":"in","label":"Original","kind":"text"}]);
        let step = Step::Board(
            json!({"updatePort":{"formationId":"fmn_plan","portId":"in","label":"Renamed"}}),
        );
        assert_eq!(
            inverse(&m, &[], &step).unwrap(),
            vec![Step::Board(
                json!({"updatePort":{"formationId":"fmn_plan","portId":"in","label":"Original"}})
            )]
        );
    }
}
