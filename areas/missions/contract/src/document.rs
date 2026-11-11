//! The HTTP documents differ from the mission and layout file schema.
use crate::{Layout, Mission, NoteEntry};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

fn rename(value: &mut Value, pairs: &[(&str, &str)]) {
    if let Some(fields) = value.as_object_mut() {
        for (from, to) in pairs {
            if let Some(v) = fields.remove(*from) {
                fields.insert((*to).into(), v);
            }
        }
    }
}
const MISSION: &[(&str, &str)] = &[
    ("inputCard", "inputCards"),
    ("formation", "formations"),
    ("gate", "gates"),
    ("end", "ends"),
    ("limit", "limits"),
    ("connection", "connections"),
];
const FORMATION: &[(&str, &str)] = &[
    ("input", "inputs"),
    ("output", "outputs"),
    ("slot", "slots"),
];
const LAYOUT: &[(&str, &str)] = &[("node", "nodes"), ("edge", "edges")];
fn reverse<'a>(pairs: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    pairs.iter().map(|(a, b)| (*b, *a)).collect()
}

/// A mission as the old HTTP API returned it, including its empty tools list.
#[derive(Clone, Debug, PartialEq)]
pub struct MissionDocument {
    pub mission: Mission,
    pub etag: Option<String>,
}
impl Serialize for MissionDocument {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut v = serde_json::to_value(&self.mission).map_err(serde::ser::Error::custom)?;
        rename(&mut v, MISSION);
        if let Some(formations) = v["formations"].as_array_mut() {
            for f in formations {
                rename(f, FORMATION);
            }
        }
        v["tools"] = Value::Array(vec![]);
        if let Some(etag) = &self.etag {
            v["etag"] = Value::String(etag.clone());
        }
        v.serialize(s)
    }
}
impl<'de> Deserialize<'de> for MissionDocument {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut v = Value::deserialize(d)?;
        let fields = v
            .as_object_mut()
            .ok_or_else(|| serde::de::Error::custom("mission must be an object"))?;
        let etag = fields
            .remove("etag")
            .map(serde_json::from_value)
            .transpose()
            .map_err(serde::de::Error::custom)?;
        if let Some(tools) = fields.remove("tools")
            && tools != serde_json::json!([])
        {
            return Err(serde::de::Error::custom("unsupported mission tools"));
        }
        if let Some(formations) = v["formations"].as_array_mut() {
            for f in formations {
                rename(f, &reverse(FORMATION));
            }
        }
        rename(&mut v, &reverse(MISSION));
        Ok(Self {
            mission: serde_json::from_value(v).map_err(serde::de::Error::custom)?,
            etag,
        })
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutDocument {
    pub layout: Layout,
    pub etag: String,
}
impl Serialize for LayoutDocument {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut v = serde_json::to_value(&self.layout).map_err(serde::ser::Error::custom)?;
        rename(&mut v, LAYOUT);
        v["etag"] = Value::String(self.etag.clone());
        v.serialize(s)
    }
}
impl<'de> Deserialize<'de> for LayoutDocument {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut v = Value::deserialize(d)?;
        let fields = v
            .as_object_mut()
            .ok_or_else(|| serde::de::Error::custom("layout must be an object"))?;
        let etag = fields
            .remove("etag")
            .ok_or_else(|| serde::de::Error::missing_field("etag"))?;
        rename(&mut v, &reverse(LAYOUT));
        Ok(Self {
            layout: serde_json::from_value(v).map_err(serde::de::Error::custom)?,
            etag: serde_json::from_value(etag).map_err(serde::de::Error::custom)?,
        })
    }
}

/// Notes are grouped by target on the wire, rather than the flat file form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesDocument {
    pub schema: i64,
    pub mission_id: String,
    pub rev: i64,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub updated_by: String,
    pub mission: Vec<Note>,
    pub elements: Vec<NoteThread>,
    pub etag: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub author: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    pub text: String,
}
impl From<NoteEntry> for Note {
    fn from(n: NoteEntry) -> Self {
        Self {
            id: n.id,
            author: n.author,
            created_at: n.created_at,
            edited_at: n.edited_at,
            text: n.text,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteThread {
    pub node_id: String,
    pub entries: Vec<Note>,
}

impl NotesDocument {
    /// Group the file's entries exactly as the HTTP notes response does.
    pub fn from_notes(notes: crate::Notes, etag: String) -> Self {
        let mut mission = Vec::new();
        let mut elements: Vec<NoteThread> = Vec::new();
        for entry in notes.entries {
            let target = entry.target.clone();
            let note = Note::from(entry);
            if target == crate::MISSION_THREAD {
                mission.push(note);
            } else if let Some(thread) = elements.iter_mut().find(|t| t.node_id == target) {
                thread.entries.push(note);
            } else {
                elements.push(NoteThread {
                    node_id: target,
                    entries: vec![note],
                });
            }
        }
        Self {
            schema: notes.schema,
            mission_id: notes.mission_id,
            rev: notes.rev,
            updated_at: if notes.updated_at.is_empty() {
                "0001-01-01T00:00:00Z".into()
            } else {
                notes.updated_at
            },
            updated_by: notes.updated_by,
            mission,
            elements,
            etag,
        }
    }
}
