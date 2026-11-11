//! Translate one HTTP mission mutation into the engine's authoring operation.
use parvi_mission_model::{
    Brief, EndUpdate, GateUpdate, InputCardUpdate, Limit, LimitUpdate, Patch, RestoredNode,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_owned()
}
fn o(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().map(str::to_owned)
}
fn n(v: &Value, k: &str) -> i64 {
    v[k].as_i64().unwrap_or_default()
}
fn b(v: &Value, k: &str) -> bool {
    v[k].as_bool().unwrap_or_default()
}
fn parse<T: DeserializeOwned>(v: &Value, k: &str) -> Result<T, String> {
    serde_json::from_value(v[k].clone()).map_err(|e| format!("{k}: {e}"))
}
fn strings(v: &Value, k: &str) -> Result<Vec<String>, String> {
    if v.get(k).is_none_or(Value::is_null) {
        Ok(Vec::new())
    } else {
        parse(v, k)
    }
}

fn fields_are_typed(v: &Value) -> Result<(), String> {
    let fields = v.as_object().ok_or("operation must be an object")?;
    for (key, value) in fields {
        if value.is_null() {
            continue;
        }
        let strings = [
            "id",
            "title",
            "goal",
            "humanChannel",
            "inputHint",
            "type",
            "keepSlotId",
            "formationId",
            "slotId",
            "agentId",
            "harness",
            "model",
            "effort",
            "beadId",
            "direction",
            "label",
            "kind",
            "portId",
            "from",
            "to",
            "previousTo",
            "criterion",
            "check",
            "checkVersion",
            "checkValue",
            "gateId",
            "outcome",
            "target",
            "whenSpent",
            "updatedBy",
        ];
        let numbers = [
            "expectedRev",
            "x",
            "y",
            "rounds",
            "seconds",
            "warnSeconds",
            "tokens",
            "index",
        ];
        let flags = ["joinIfOccupied", "removePreviousInput"];
        if strings.contains(&key.as_str()) && !value.is_string()
            || numbers.contains(&key.as_str()) && value.as_i64().is_none()
            || flags.contains(&key.as_str()) && !value.is_boolean()
        {
            return Err(format!("{key} has the wrong type"));
        }
    }
    Ok(())
}

pub fn decode(body: &Value) -> Result<Patch, String> {
    let map = body.as_object().ok_or("expected an object")?;
    let ops: Vec<_> = map
        .keys()
        .filter(|k| !["expectedRev", "updatedBy"].contains(&k.as_str()))
        .collect();
    if ops.len() != 1 {
        let names: Vec<_> = ops.iter().map(|x| x.as_str()).collect();
        let joined = match names.as_slice() {
            [] => "none".to_owned(),
            [one] => (*one).to_owned(),
            [a, b] => format!("{a} and {b}"),
            many => format!(
                "{} and {}",
                many[..many.len() - 1].join(", "),
                many[many.len() - 1]
            ),
        };
        return Err(format!(
            "a mission patch names one operation, and this one names {joined}: send them one at a time"
        ));
    }
    let op = ops[0].as_str();
    let v = &body[op];
    if op != "title" {
        fields_are_typed(v)?;
    }
    let id = || s(v, "id");
    let fid = || s(v, "formationId");
    let xy = || (n(v, "x"), n(v, "y"));
    Ok(match op {
        "title" => Patch::Title(v.as_str().ok_or("title must be a string")?.into()),
        "createInputCard" => {
            let (x, y) = xy();
            Patch::CreateInputCard {
                title: s(v, "title"),
                goal: s(v, "goal"),
                files: strings(v, "files")?,
                human_channel: s(v, "humanChannel"),
                x,
                y,
            }
        }
        "updateInputCard" => Patch::UpdateInputCard(InputCardUpdate {
            id: id(),
            title: o(v, "title"),
            goal: o(v, "goal"),
            files: v.get("files").map(|_| parse(v, "files")).transpose()?,
            input_hint: o(v, "inputHint"),
            human_channel: o(v, "humanChannel"),
            inputs: v.get("inputs").map(|_| parse(v, "inputs")).transpose()?,
            outputs: v.get("outputs").map(|_| parse(v, "outputs")).transpose()?,
        }),
        "deleteInputCard" => Patch::DeleteInputCard { id: id() },
        "createFormation" => {
            let (x, y) = xy();
            Patch::CreateFormation {
                kind: s(v, "type"),
                title: s(v, "title"),
                x,
                y,
            }
        }
        "deleteFormation" => Patch::DeleteFormation { id: id() },
        "editFormation" => Patch::EditFormation {
            id: id(),
            title: s(v, "title"),
            brief: parse(v, "brief")?,
        },
        "updateFormation" => Patch::UpdateFormation {
            id: id(),
            title: o(v, "title"),
            device: o(v, "device"),
        },
        "setFormationType" => Patch::SetFormationType {
            id: id(),
            kind: s(v, "type"),
            keep_slot_id: s(v, "keepSlotId"),
            slots: v.get("slots").map(|_| parse(v, "slots")).transpose()?,
        },
        "assignSlot" => Patch::AssignSlot {
            formation_id: fid(),
            slot_id: s(v, "slotId"),
            agent_id: s(v, "agentId"),
            harness: s(v, "harness"),
            model: s(v, "model"),
            effort: s(v, "effort"),
        },
        "swapSlots" => Patch::SwapSlots {
            from_formation: s(v, "fromFormation"),
            from_slot: s(v, "fromSlot"),
            to_formation: s(v, "toFormation"),
            to_slot: s(v, "toSlot"),
        },
        "makeController" => Patch::MakeController {
            formation_id: fid(),
            slot_id: s(v, "slotId"),
        },
        "setBrief" => Patch::SetBrief {
            formation_id: fid(),
            brief: Brief {
                repository: s(v, "repository"),
                goal: s(v, "goal"),
                work: s(v, "beadId"),
                files: strings(v, "files")?,
                links: strings(v, "links")?,
            },
        },
        "clearBrief" => Patch::ClearBrief {
            formation_id: fid(),
        },
        "addPort" => Patch::AddPort {
            formation_id: fid(),
            direction: s(v, "direction"),
            label: s(v, "label"),
            kind: s(v, "kind"),
        },
        "updatePort" => Patch::UpdatePort {
            formation_id: fid(),
            port_id: s(v, "portId"),
            label: o(v, "label"),
            kind: o(v, "kind"),
        },
        "removePort" => Patch::RemovePort {
            formation_id: fid(),
            port_id: s(v, "portId"),
        },
        "restorePort" => Patch::RestorePort {
            formation_id: fid(),
            direction: s(v, "direction"),
            port: parse(v, "port")?,
            index: v["index"].as_u64().map(|n| n as usize),
            connections: strings_or(v, "connections")?,
        },
        "wireConnection" => Patch::WireConnection {
            id: id(),
            from: s(v, "from"),
            to: s(v, "to"),
            join_if_occupied: b(v, "joinIfOccupied"),
        },
        "unwireConnection" => Patch::UnwireConnection {
            from: s(v, "from"),
            to: s(v, "to"),
        },
        "rewireConnection" => Patch::RewireConnection {
            from: s(v, "from"),
            previous_to: s(v, "previousTo"),
            to: s(v, "to"),
            join_if_occupied: b(v, "joinIfOccupied"),
            remove_previous_input: b(v, "removePreviousInput"),
        },
        "createGate" => {
            let (x, y) = xy();
            let mut kinds = strings(v, "kinds")?;
            if kinds.is_empty() {
                kinds.push("human".into());
            }
            Patch::CreateGate {
                title: s(v, "title"),
                kinds,
                criterion: s(v, "criterion"),
                check: s(v, "check"),
                check_version: s(v, "checkVersion"),
                check_value: s(v, "checkValue"),
                files: strings(v, "files")?,
                x,
                y,
            }
        }
        "updateGate" => Patch::UpdateGate(GateUpdate {
            id: id(),
            title: o(v, "title"),
            kinds: v.get("kinds").map(|_| parse(v, "kinds")).transpose()?,
            criterion: o(v, "criterion"),
            check: o(v, "check"),
            check_version: o(v, "checkVersion"),
            check_value: o(v, "checkValue"),
            files: v.get("files").map(|_| parse(v, "files")).transpose()?,
        }),
        "deleteGate" => Patch::DeleteGate { id: id() },
        "setGateJudge" => Patch::SetGateJudge {
            gate_id: s(v, "gateId"),
            chain: strings(v, "chain")?,
        },
        "detachGateJudge" => Patch::DetachGateJudge {
            gate_id: s(v, "gateId"),
        },
        "createEnd" => {
            let (x, y) = xy();
            Patch::CreateEnd {
                title: s(v, "title"),
                outcome: s(v, "outcome"),
                x,
                y,
            }
        }
        "updateEnd" => Patch::UpdateEnd(EndUpdate {
            id: id(),
            title: o(v, "title"),
            outcome: o(v, "outcome"),
        }),
        "deleteEnd" => Patch::DeleteEnd { id: id() },
        "createLimit" => {
            let (x, y) = xy();
            Patch::CreateLimit {
                limit: Limit {
                    id: String::new(),
                    title: s(v, "title"),
                    target: s(v, "target"),
                    rounds: v["rounds"].as_i64(),
                    seconds: v["seconds"].as_i64(),
                    warn_seconds: v["warnSeconds"].as_i64(),
                    tokens: v["tokens"].as_i64(),
                    when_spent: s(v, "whenSpent"),
                },
                x,
                y,
            }
        }
        "updateLimit" => Patch::UpdateLimit(LimitUpdate {
            id: id(),
            title: o(v, "title"),
            target: o(v, "target"),
            rounds: v["rounds"].as_i64(),
            seconds: v["seconds"].as_i64(),
            warn_seconds: v["warnSeconds"].as_i64(),
            tokens: v["tokens"].as_i64(),
            when_spent: o(v, "whenSpent"),
        }),
        "deleteLimit" => Patch::DeleteLimit { id: id() },
        "restoreNode" => {
            if ["inputCard", "formation", "gate", "end", "limit"]
                .iter()
                .filter(|key| v.get(**key).is_some_and(|n| !n.is_null()))
                .count()
                != 1
            {
                return Err("restoreNode requires exactly one node".into());
            }
            let node = if v.get("inputCard").is_some_and(|n| !n.is_null()) {
                RestoredNode::InputCard(parse(v, "inputCard")?)
            } else if v.get("formation").is_some_and(|n| !n.is_null()) {
                let mut formation = v["formation"].clone();
                let fields = formation
                    .as_object_mut()
                    .ok_or("formation must be an object")?;
                for (from, to) in [
                    ("inputs", "input"),
                    ("outputs", "output"),
                    ("slots", "slot"),
                ] {
                    if let Some(value) = fields.remove(from) {
                        fields.insert(to.into(), value);
                    }
                }
                RestoredNode::Formation(
                    serde_json::from_value(formation).map_err(|e| format!("formation: {e}"))?,
                )
            } else if v.get("gate").is_some_and(|n| !n.is_null()) {
                RestoredNode::Gate(parse(v, "gate")?)
            } else if v.get("end").is_some_and(|n| !n.is_null()) {
                RestoredNode::End(parse(v, "end")?)
            } else if v.get("limit").is_some_and(|n| !n.is_null()) {
                RestoredNode::Limit(parse(v, "limit")?)
            } else {
                return Err("restoreNode requires a node".into());
            };
            let (x, y) = xy();
            Patch::RestoreNode {
                node,
                connections: strings_or(v, "connections")?,
                index: v["index"].as_u64().map(|n| n as usize),
                x,
                y,
            }
        }
        _ => return Err(format!("unknown operation {op}")),
    })
}
fn strings_or<T: DeserializeOwned>(v: &Value, k: &str) -> Result<Vec<T>, String> {
    if v.get(k).is_none() {
        Ok(Vec::new())
    } else {
        parse(v, k)
    }
}

/// Keep duplicate operation keys visible: a JSON object naming the same edit
/// twice still names two operations and must never silently discard one.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(transparent)]
pub struct UniquePatch(pub Value);
impl<'de> serde::Deserialize<'de> for UniquePatch {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniquePatch;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a mission patch object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                use serde::de::Error;
                let mut result = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    if result.contains_key(&key) {
                        return Err(M::Error::custom(format!(
                            "a mission patch repeats {key}: send one operation at a time"
                        )));
                    }
                    result.insert(key, value);
                }
                Ok(UniquePatch(Value::Object(result)))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn malformed_operation_fields_are_not_silently_dropped() {
        assert!(decode(&json!({"createEnd":{"outcome":42}})).is_err());
        assert!(decode(&json!({"createFormation":{"x":"100"}})).is_err());
        assert!(decode(&json!({"wireConnection":{"joinIfOccupied":"yes"}})).is_err());
    }
    #[test]
    fn gate_without_kinds_defaults_to_human() {
        let Patch::CreateGate { kinds, .. } =
            decode(&json!({"createGate":{"title":"Signoff"}})).unwrap()
        else {
            panic!("expected gate")
        };
        assert_eq!(kinds, ["human"]);
    }
    #[test]
    fn restore_formation_reads_the_public_json_shape() {
        let body = json!({"restoreNode":{"formation":{"id":"f","type":"solo","title":"Work","inputs":[{"id":"in","label":"Input"}],"outputs":[],"slots":[{"id":"s","label":"Agent","controller":false}]}}});
        let Patch::RestoreNode {
            node: RestoredNode::Formation(formation),
            ..
        } = decode(&body).unwrap()
        else {
            panic!("expected a formation restore")
        };
        assert_eq!(formation.inputs[0].id, "in");
        assert_eq!(formation.slots[0].id, "s");
        let mut invalid = body;
        invalid["restoreNode"]["gate"] = json!({"id":"g"});
        assert!(decode(&invalid).is_err());
    }
}
