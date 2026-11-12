//! CLI authoring operations, shared by the node's MCP and CLI commands.
mod edit;
use crate::Missions;
use anyhow::{Context, Result, bail};
use parvi_core::Error;
use parvi_missions_contract::{self as wire, LayoutPatch, Precondition};
use serde_json::{Value, json};
use std::collections::HashMap;

struct Args {
    pos: Vec<String>,
    flags: HashMap<String, Vec<String>>,
}
impl Args {
    fn arg(&self, i: usize) -> Result<&str> {
        self.pos
            .get(i)
            .map(String::as_str)
            .context("missing argument")
    }
    fn flag(&self, k: &str) -> Option<&str> {
        self.flags.get(k).and_then(|v| v.last()).map(String::as_str)
    }
    fn has(&self, k: &str) -> bool {
        self.flags.contains_key(k)
    }
    fn files(&self) -> Vec<String> {
        self.flags.get("file").cloned().unwrap_or_default()
    }
}
fn text(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").into()
}
fn select(m: &Value, list: &str, selector: &str) -> Result<String> {
    let nodes = m[list].as_array().context("node returned no list")?;
    let matches: Vec<_> = nodes
        .iter()
        .filter(|n| n["id"] == selector || n["title"] == selector)
        .collect();
    match matches.as_slice() {
        [one] => Ok(text(one, "id")),
        [] => bail!("{selector:?} not found"),
        _ => bail!("{selector:?} is ambiguous"),
    }
}
fn slot_id(m: &Value, formation_id: &str, selector: &str) -> Result<String> {
    let formation = m["formations"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["id"] == formation_id)
        .context("formation not found")?;
    let matches: Vec<_> = formation["slots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["id"] == selector || s["label"] == selector)
        .collect();
    match matches.as_slice() {
        [slot] => Ok(text(slot, "id")),
        [] => bail!("slot {selector:?} not found"),
        _ => bail!("slot {selector:?} is ambiguous"),
    }
}
/// Which end of a connection an endpoint names, so a bare node can stand for
/// its only port on that side.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    From,
    To,
}
/// A step with one seat needs no --slot; one with several names them.
fn only_slot(m: &Value, formation_id: &str) -> Result<String> {
    let formation = m["formations"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| f["id"] == formation_id)
        .context("step not found")?;
    let slots: Vec<_> = formation["slots"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    match slots.as_slice() {
        [slot] => Ok(text(slot, "id")),
        _ => bail!(
            "this step has several seats; name one with --slot: {}",
            slots
                .iter()
                .map(|s| text(s, "label"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
fn endpoint(m: &Value, endpoint: &str, side: Side) -> Result<String> {
    let (node, port) = endpoint.split_once(':').unwrap_or((endpoint, ""));
    let mut found = Vec::new();
    for key in ["inputCards", "formations", "gates", "ends"] {
        for item in m[key].as_array().into_iter().flatten() {
            if item["id"] == node || item["title"] == node {
                found.push((key, item))
            }
        }
    }
    let (list, node) = match found.as_slice() {
        [found] => *found,
        [] => bail!("node {node:?} not found"),
        _ => bail!("node {node:?} is ambiguous"),
    };
    let id = text(node, "id");
    let port = if !port.is_empty() {
        port.to_owned()
    } else if list == "formations" {
        let key = if side == Side::From {
            "outputs"
        } else {
            "inputs"
        };
        let ports: Vec<_> = node[key].as_array().into_iter().flatten().collect();
        match ports.as_slice() {
            [one] => text(one, "id"),
            [] => bail!("{:?} has no {key}; add one first", text(node, "title")),
            _ => bail!(
                "{:?} has several {key}; name one as {}:<port>, one of {}",
                text(node, "title"),
                text(node, "title"),
                ports
                    .iter()
                    .map(|p| text(p, "label"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    } else if list == "ends" || (list == "gates" && side == Side::To) {
        "in".to_owned()
    } else if list == "gates" {
        bail!(
            "a gate has two ways out; name one as {}:pass or {}:fail",
            text(node, "title"),
            text(node, "title")
        )
    } else {
        "out".to_owned()
    };
    let port = port.as_str();
    if let Some(ports) = node
        .get("slots")
        .and_then(|_| node.get("inputs"))
        .and_then(Value::as_array)
    {
        let matches: Vec<_> = ports
            .iter()
            .chain(node["outputs"].as_array().into_iter().flatten())
            .filter(|p| p["id"] == port || p["label"] == port)
            .collect();
        return match matches.as_slice() {
            [p] => Ok(format!("{id}:{}", text(p, "id"))),
            [] => bail!("port {port:?} not found"),
            _ => bail!("port {port:?} is ambiguous"),
        };
    }
    Ok(format!("{id}:{port}"))
}
fn selected_input(m: &Value, selector: Option<&str>) -> Result<String> {
    if let Some(s) = selector {
        return select(m, "inputCards", s);
    }
    let cards = m["inputCards"].as_array().context("missing input cards")?;
    if cards.len() != 1 {
        bail!("mission needs exactly one Input card")
    }
    Ok(text(&cards[0], "id"))
}
fn value_flags(args: &Args, fields: &[(&str, &str)]) -> Value {
    let mut v = json!({});
    for (flag, key) in fields {
        if let Some(value) = args.flag(flag) {
            v[*key] = json!(value)
        }
    }
    v
}
fn set_node_flag(v: &mut Value, args: &Args, key: &str) {
    if args.has("file") {
        v[key] = json!(
            args.files()
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        )
    }
}

pub const NAMES: &[&str] = &[
    "mission_arrange",
    "mission_rename",
    "mission_move",
    "mission_create",
    "mission_update",
    "mission_delete",
    "mission_input",
    "mission_output",
    "mission_wire",
    "formation_list",
    "formation_inspect",
    "formation_create",
    "formation_rename",
    "formation_place",
    "formation_set_type",
    "formation_set_slots",
    "formation_assign",
    "formation_unassign",
    "formation_set_brief",
    "formation_add_input",
    "formation_add_output",
    "formation_set_port",
    "formation_wire",
    "formation_unwire",
    "formation_delete",
    "gate_create",
    "gate_update",
    "gate_judge",
    "gate_delete",
    "end_create",
    "end_update",
    "end_delete",
    "limit_create",
    "limit_update",
    "limit_delete",
];

pub fn description(name: &str) -> &'static str {
    match name {
        "mission_arrange" => "Replace saved canvas positions with compact reading rows.",
        "mission_rename" => "Change the mission's title, as the app's Rename does. The slug stays.",
        "mission_move" => {
            "Move one card on the canvas: the Input card, a step, gate, End node or Limit card, by id or title."
        }
        "mission_create" => {
            "Add the Input card to a mission that has none. Create the mission itself with parvi mission new <slug>."
        }
        "mission_update" => {
            "Change the Input card. Only the flags you give change it, and an empty value clears that field."
        }
        "mission_delete" => {
            "Archive the mission definition, layout and its notes; run history remains. List archives with mission archives and remove one with mission purge-archive."
        }
        "mission_input" => {
            "List the inputs a run supplies, or with a name declare, change or --delete that input. A mission that declares none takes one required text input, brief. A new input is optional text unless the flags say otherwise, and only the flags you give change an existing one; step briefs reference it as {name}."
        }
        "mission_output" => {
            "List the outputs a successful run promises, or with a name declare, change or --delete that output. Each is bound with --from to one step output port of the same kind; when the run reaches a done End its value is that port's latest accepted output, a snapshot path for a file or folder. A missing required output fails the run. A new output is optional text unless the flags say otherwise, and only the flags you give change an existing one."
        }
        "mission_wire" => "Wire the Input card to a step.",
        "formation_list" => "List the steps with their slots and staffing.",
        "formation_inspect" => "Print one step with its slots, ports, brief and connections.",
        "formation_create" => "Add a step. It is solo unless you name another type.",
        "formation_rename" => "Change the step's title.",
        "formation_place" => {
            "Choose where the step runs: a device and environment as the fleet names them, or home for wherever the run starts."
        }
        "formation_set_type" => {
            "Change the step's type. --keep-slot names the slot a change to solo keeps."
        }
        "formation_set_slots" => "Replace the slot list, preserving staffing for retained IDs.",
        "formation_assign" => {
            "Staff a slot. A slot owns its harness, model and effort; --role adds optional role text, and a slot without one is a vanilla agent. --character stamps a saved character's role, harness, model and effort, each replaced by its own flag when given; the slot keeps no link to it."
        }
        "formation_unassign" => "Empty a slot.",
        "formation_set_brief" => "Write the step's brief.",
        "formation_add_input" => "Add an input port.",
        "formation_add_output" => {
            "Add an output port. A file or folder port's ref is an absolute path; Archon snapshots it into the attempt directory when the output is accepted."
        }
        "formation_set_port" => {
            "Relabel a port, or set what an output port carries. <port> is the port's ID or label."
        }
        "formation_wire" => {
            "Connect two ports. --join adds an input when the target is already fed."
        }
        "formation_unwire" => "Remove a connection.",
        "formation_delete" => "Remove a step, its connections and its Limit cards.",
        "gate_create" => "Add a gate. It is human unless --kinds says otherwise.",
        "gate_update" => {
            "Change a gate. Only the fields given change, and an empty value clears one."
        }
        "gate_judge" => "Attach a judge chain of formations, or detach it.",
        "gate_delete" => {
            "Remove a gate and its routes, including its judge attachment; judge formations remain."
        }
        "end_create" => {
            "Add an End node. --outcome is done (the default) or rejected. Wire a route into it with parvi formation wire <mission> <node:port> <end-id>:in."
        }
        "end_update" => "Change an End node's title or outcome.",
        "end_delete" => "Remove an End node.",
        "limit_create" => {
            "Add a Limit card. --when-spent fail ends the run as failed, keeping attempt evidence. By default the run blocks; run resume --grant gives one more round, or the card's time or tokens again, and a step's card also takes run resume --pick-best, which picks the best attempt the step's gate judged. With --when-spent best a step's card picks the best attempt its gate judged and goes on through the gate's pass route; the step needs one judge or human gate that only it feeds and whose fail returns to it. Tokens are approximate: input not read from the cache, cache writes included, plus output, subagents included."
        }
        "limit_update" => {
            "Change a Limit card. Only the flags you give change; an empty --rounds, --time, --warn or --tokens clears that knob and an empty --target unwires the card."
        }
        "limit_delete" => "Remove a Limit card.",
        _ => unreachable!(),
    }
}

fn allowed(name: &str) -> &'static [&'static str] {
    match name {
        "mission_arrange" => &["mission"],
        "mission_rename" => &["mission", "title", "updated_by"],
        "mission_move" => &["mission", "card", "x", "y"],
        "mission_create" => &[
            "mission",
            "file",
            "goal",
            "human_channel",
            "title",
            "updated_by",
            "x",
            "y",
        ],
        "mission_update" => &[
            "mission",
            "input",
            "file",
            "goal",
            "human_channel",
            "input_hint",
            "title",
            "updated_by",
        ],
        "mission_delete" => &["mission"],
        "mission_input" => &[
            "mission",
            "name",
            "delete",
            "description",
            "kind",
            "optional",
            "required",
            "updated_by",
        ],
        "mission_output" => &[
            "mission",
            "name",
            "delete",
            "description",
            "from",
            "kind",
            "optional",
            "required",
            "updated_by",
        ],
        "mission_wire" => &["mission", "input", "to", "updated_by"],
        "formation_list" => &["mission"],
        "formation_inspect" => &["mission", "formation"],
        "formation_create" => &["mission", "type", "title", "updated_by", "x", "y"],
        "formation_rename" => &["mission", "formation", "title", "updated_by"],
        "formation_place" => &["mission", "formation", "place", "updated_by"],
        "formation_set_type" => &["mission", "formation", "type", "keep_slot", "updated_by"],
        "formation_set_slots" => &["mission", "formation", "slot", "controller", "updated_by"],
        "formation_assign" => &[
            "mission",
            "formation",
            "character",
            "effort",
            "harness",
            "model",
            "role",
            "slot",
            "updated_by",
        ],
        "formation_unassign" => &["mission", "formation", "slot", "updated_by"],
        "formation_set_brief" => &[
            "mission",
            "formation",
            "bead",
            "file",
            "goal",
            "link",
            "updated_by",
        ],
        "formation_add_input" => &["mission", "formation", "label", "updated_by", "kind"],
        "formation_add_output" => &["mission", "formation", "kind", "label", "updated_by"],
        "formation_set_port" => &[
            "mission",
            "formation",
            "port",
            "kind",
            "label",
            "updated_by",
        ],
        "formation_wire" => &["mission", "from", "to", "join", "updated_by"],
        "formation_unwire" => &["mission", "from", "to", "updated_by"],
        "formation_delete" => &["mission", "formation", "updated_by"],
        "gate_create" => &[
            "mission",
            "check",
            "check_value",
            "check_version",
            "criterion",
            "file",
            "kinds",
            "title",
            "updated_by",
            "x",
            "y",
        ],
        "gate_update" => &[
            "mission",
            "gate",
            "check",
            "check_value",
            "check_version",
            "clear_check",
            "criterion",
            "file",
            "kinds",
            "title",
            "updated_by",
        ],
        "gate_judge" => &["mission", "gate", "chain", "detach", "updated_by"],
        "gate_delete" => &["mission", "gate", "updated_by"],
        "end_create" => &["mission", "outcome", "title", "updated_by", "x", "y"],
        "end_update" => &["mission", "end", "outcome", "title", "updated_by"],
        "end_delete" => &["mission", "end", "updated_by"],
        "limit_create" => &[
            "mission",
            "rounds",
            "target",
            "time",
            "title",
            "tokens",
            "updated_by",
            "warn",
            "when_spent",
            "x",
            "y",
        ],
        "limit_update" => &[
            "mission",
            "limit",
            "rounds",
            "target",
            "time",
            "title",
            "tokens",
            "updated_by",
            "warn",
            "when_spent",
        ],
        "limit_delete" => &["mission", "limit", "updated_by"],
        _ => &[],
    }
}

/// The same named arguments become CLI positionals and flags through node-kit.
pub fn schema(name: &str) -> Value {
    let s = json!({"type":"string"});
    let b = json!({"type":"boolean"});
    let i = json!({"type":"integer"});
    let mut props = serde_json::Map::new();
    for key in [
        "mission",
        "formation",
        "gate",
        "end",
        "limit",
        "input",
        "name",
        "card",
        "title",
        "place",
        "kind",
        "port",
        "from",
        "to",
        "type",
        "goal",
        "human_channel",
        "input_hint",
        "criterion",
        "check",
        "check_version",
        "check_value",
        "outcome",
        "target",
        "when_spent",
        "description",
        "repository",
        "bead",
        "role",
        "harness",
        "model",
        "effort",
        "label",
        "controller",
        "keep_slot",
        "chain",
        "kinds",
        "rounds",
        "tokens",
        "time",
        "warn",
        "updated_by",
        "character",
    ] {
        props.insert(key.into(), s.clone());
    }
    for key in [
        "required",
        "optional",
        "delete",
        "join",
        "detach",
        "clear_check",
    ] {
        props.insert(key.into(), b.clone());
    }
    for key in ["x", "y"] {
        props.insert(key.into(), i.clone());
    }
    for key in ["file", "link", "slot"] {
        props.insert(key.into(), json!({"type":"array","items":s}));
    }
    if matches!(name, "formation_assign" | "formation_unassign") {
        props.insert("slot".into(), s.clone());
    }
    props.retain(|key, _| allowed(name).contains(&key.as_str()));
    let props = Value::Object(props);
    let required: &[&str] = match name {
        "mission_rename" => &["mission", "title"],
        "mission_move" => &["mission", "card"],
        "mission_wire" => &["mission", "to"],
        "formation_create" => &["mission"],
        "formation_rename" => &["mission", "formation", "title"],
        "formation_place" => &["mission", "formation", "place"],
        "formation_set_type" => &["mission", "formation", "type"],
        "formation_set_port" => &["mission", "formation", "port"],
        "formation_wire" | "formation_unwire" => &["mission", "from", "to"],
        "formation_inspect"
        | "formation_set_slots"
        | "formation_assign"
        | "formation_unassign"
        | "formation_set_brief"
        | "formation_add_input"
        | "formation_add_output"
        | "formation_delete" => &["mission", "formation"],
        "gate_update" | "gate_judge" | "gate_delete" => &["mission", "gate"],
        "end_update" | "end_delete" => &["mission", "end"],
        "limit_update" | "limit_delete" => &["mission", "limit"],
        _ => &["mission"],
    };
    parvi_node_kit::args::object(props, required)
}
fn bad(error: impl std::fmt::Display) -> Error {
    Error::new("bad_request", error.to_string())
}
fn required<'a>(args: &'a Value, key: &str) -> std::result::Result<&'a str, Error> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad(format!("{key} is required")))
}
fn args(name: &str, value: &Value) -> std::result::Result<Args, Error> {
    let fields = value
        .as_object()
        .ok_or_else(|| bad("arguments must be an object"))?;
    if let Some(key) = fields
        .keys()
        .find(|key| !allowed(name).contains(&key.as_str()))
    {
        return Err(bad(format!("unknown argument {key}")));
    }
    let mission = required(value, "mission")?.to_owned();
    let mut pos = vec![mission];
    let fields: &[&str] = match name {
        "mission_rename" => &["title"],
        "mission_move" => &["card"],
        "mission_update" => &["input"],
        "mission_input" | "mission_output" => &["name"],
        "formation_create" => &["type"],
        "formation_rename" => &["formation", "title"],
        "formation_place" => &["formation", "place"],
        "formation_set_type" => &["formation", "type"],
        "formation_set_port" => &["formation", "port"],
        "formation_wire" | "formation_unwire" => &["from", "to"],
        "formation_inspect"
        | "formation_set_slots"
        | "formation_assign"
        | "formation_unassign"
        | "formation_set_brief"
        | "formation_add_input"
        | "formation_add_output"
        | "formation_delete" => &["formation"],
        "gate_update" | "gate_judge" | "gate_delete" => &["gate"],
        "end_update" | "end_delete" => &["end"],
        "limit_update" | "limit_delete" => &["limit"],
        _ => &[],
    };
    if name == "mission_wire" {
        if let Some(input) = value.get("input").and_then(Value::as_str) {
            pos.push(input.into());
        }
        pos.push(required(value, "to")?.into());
    } else {
        for key in fields {
            if let Some(v) = value.get(*key) {
                pos.push(
                    v.as_str()
                        .ok_or_else(|| bad(format!("{key} must be a string")))?
                        .into(),
                );
            }
        }
    }
    let required = schema(name)["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for key in required
        .iter()
        .filter_map(Value::as_str)
        .filter(|k| *k != "x" && *k != "y")
    {
        required_value(value, key)?;
    }
    let mut flags = HashMap::new();
    for (key, v) in value
        .as_object()
        .ok_or_else(|| bad("arguments must be an object"))?
    {
        if v.is_null() {
            continue;
        }
        let key = key.replace('_', "-");
        let values: Vec<String> = match v {
            Value::String(s) => vec![s.clone()],
            Value::Number(n) => vec![n.to_string()],
            Value::Bool(true) => vec!["true".into()],
            Value::Bool(false) => continue,
            Value::Array(v) => v
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| bad("array items must be strings"))
                })
                .collect::<std::result::Result<_, _>>()?,
            _ => return Err(bad(format!("invalid {key}"))),
        };
        flags.insert(key, values);
    }
    Ok(Args { pos, flags })
}
fn required_value(args: &Value, key: &str) -> std::result::Result<(), Error> {
    required(args, key).map(|_| ())
}

pub fn call(m: &Missions, name: &str, input: &Value) -> std::result::Result<Value, Error> {
    let mut args = args(name, input)?;
    if name == "formation_assign"
        && let Some(character) = args.flag("character").map(str::to_owned)
    {
        let found = m
            .store
            .characters()?
            .into_iter()
            .find(|c| c.id == character)
            .ok_or_else(|| Error::new("not_found", "character not found"))?;
        for (flag, value) in [
            ("role", found.agent_id),
            ("harness", found.harness),
            ("model", found.model),
            ("effort", found.effort),
        ] {
            if !args.has(flag) {
                args.flags.insert(flag.into(), vec![value]);
            }
        }
    }
    let selector = required(input, "mission")?;
    let current = m.store.mission_edit(selector)?;
    let before = super::commands::document(current.mission.clone(), current.etag.clone())?;
    if name == "formation_list" {
        return Ok(json!({"node":m.node,"formations":before["formations"],"etag":current.etag}));
    }
    if name == "formation_inspect" {
        let id = select(&before, "formations", args.arg(1).map_err(bad)?).map_err(bad)?;
        let formation = before["formations"]
            .as_array()
            .and_then(|v| v.iter().find(|n| n["id"] == id))
            .ok_or_else(|| bad("formation not found"))?;
        return Ok(json!({"node":m.node,"formation":formation,"etag":current.etag}));
    }
    if name == "mission_delete" {
        let deleted = m.store.delete_mission(
            selector,
            Precondition {
                rev: current.mission.rev,
                etag: current.etag,
            },
        )?;
        m.changed();
        return Ok(
            json!({"node":m.node,"deletion":{"id":deleted.id,"slug":deleted.slug,"title":deleted.title,"archiveId":deleted.archive_id}}),
        );
    }
    if name == "mission_arrange" || name == "mission_move" {
        let current_layout = m.store.layout_edit(selector)?;
        let nodes = if name == "mission_arrange" {
            parvi_mission_engine::arrange(&current.mission, &current_layout.layout).nodes
        } else {
            let card = required(input, "card")?;
            let matches: Vec<_> = ["inputCards", "formations", "gates", "ends", "limits"]
                .iter()
                .filter_map(|list| select(&before, list, card).ok())
                .collect();
            let id = match matches.as_slice() {
                [id] => id.clone(),
                [] => return Err(Error::new("not_found", "card not found")),
                _ => return Err(bad("card is ambiguous")),
            };
            let number = |key| {
                input
                    .get(key)
                    .and_then(Value::as_i64)
                    .ok_or_else(|| bad(format!("{key} is required as an integer")))
            };
            vec![wire::LayoutNode {
                id,
                x: number("x")?,
                y: number("y")?,
            }]
        };
        let result =
            m.store
                .patch_layout(selector, &current_layout.etag, LayoutPatch::Nodes(nodes))?;
        m.changed();
        return Ok(json!({"node":m.node,"layout":super::commands::layout(result)?}));
    }
    if (name == "mission_input" || name == "mission_output") && args.pos.len() == 1 {
        let id = selected_input(&before, None).map_err(bad)?;
        let card = before["inputCards"]
            .as_array()
            .and_then(|v| v.iter().find(|c| c["id"] == id))
            .ok_or_else(|| bad("input card not found"))?;
        let key = if name == "mission_input" {
            "inputs"
        } else {
            "outputs"
        };
        let mut items = card[key].as_array().cloned().unwrap_or_default();
        if key == "inputs" && items.is_empty() {
            let mut brief = json!({"name":"brief","kind":"text","required":true});
            if let Some(hint) = card["inputHint"].as_str() {
                brief["description"] = json!(hint);
            }
            items.push(brief);
        }
        return Ok(json!({"node":m.node,"mission":selector,key:items}));
    }
    let (operation, mut fields, created) = if name == "mission_rename" {
        ("title", json!(required(input, "title")?), None)
    } else {
        let (noun, verb) = name.split_once('_').ok_or_else(|| bad("invalid command"))?;
        let verb = verb.replace('_', "-");
        edit::operation(noun, &verb, &args, &before).map_err(bad)?
    };
    if created.is_some() && input.get("x").is_none() && input.get("y").is_none() {
        let layout = m.store.layout_edit(selector)?;
        let preferred = if name == "formation_create" { 120 } else { 0 };
        let position = parvi_mission_engine::free_layout_position(
            &current.mission,
            Some(&layout.layout),
            preferred,
            preferred,
        )?;
        fields["x"] = json!(position.x);
        fields["y"] = json!(position.y);
    }
    // A mission still laid out as Arrange lays it out stays that way as an
    // agent adds to it; one a person has placed by hand keeps their places.
    let arranged = {
        let layout = m.store.layout_edit(selector)?.layout;
        let place = |nodes: &[wire::LayoutNode]| {
            let mut v: Vec<_> = nodes.iter().map(|n| (n.id.clone(), n.x, n.y)).collect();
            v.sort();
            v
        };
        layout.nodes.is_empty()
            || place(&parvi_mission_engine::arrange(&current.mission, &layout).nodes)
                == place(&layout.nodes)
    };
    let change = json!({operation:fields});
    let patch = wire::patch::decode(&change).map_err(bad)?;
    let result = m.store.patch(
        selector,
        Precondition {
            rev: current.mission.rev,
            etag: current.etag,
        },
        patch,
        args.flag("updated-by").unwrap_or("agent:parvi"),
    )?;
    if arranged {
        let layout = m.store.layout_edit(selector)?;
        let nodes = parvi_mission_engine::arrange(&result.mission, &layout.layout).nodes;
        m.store
            .patch_layout(selector, &layout.etag, LayoutPatch::Nodes(nodes))?;
    }
    m.changed();
    let after = super::commands::document(result.mission, result.etag)?;
    Ok(brief_answer(&m.node, &before, &after, created))
}

/// An edit answers with what it changed, not the whole mission: the cards
/// and connections that are new or different, and the ids of those removed.
/// `parvi mission inspect` shows the whole.
fn brief_answer(node: &str, before: &Value, after: &Value, created: Option<&str>) -> Value {
    let mut answer = json!({"node":node,"mission":after["slug"],"title":after["title"],"rev":after["rev"],"etag":after["etag"]});
    let mut changed = Vec::new();
    let mut removed = Vec::new();
    for (key, items) in after.as_object().into_iter().flatten() {
        let Some(items) = items.as_array() else {
            continue;
        };
        let old = before[key].as_array().cloned().unwrap_or_default();
        changed.extend(items.iter().filter(|item| !old.contains(item)).cloned());
        removed.extend(
            old.iter()
                .filter(|o| {
                    o.get("id").is_some() && !items.iter().any(|item| item["id"] == o["id"])
                })
                .map(|o| o["id"].clone()),
        );
        if let Some(created) = created
            && key == created_list(created)
            && let Some(item) = items
                .iter()
                .find(|item| !old.iter().any(|o| o["id"] == item["id"]))
        {
            answer[created] = item.clone();
        }
    }
    if !changed.is_empty() {
        answer["changed"] = json!(changed);
    }
    if !removed.is_empty() {
        answer["removed"] = json!(removed);
    }
    answer
}
fn created_list(key: &str) -> &'static str {
    match key {
        "inputCard" => "inputCards",
        "formation" => "formations",
        "gate" => "gates",
        "end" => "ends",
        "limit" => "limits",
        _ => unreachable!(),
    }
}
