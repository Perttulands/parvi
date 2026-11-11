//! A mission: one schema-1 TOML graph file, `<slug>.mission.toml`.
//!
//! The file is read only as strict TOML of its schema: a repeated key, a
//! value of the wrong type or a field the schema does not have refuses the
//! whole file with `invalid_definition_source`. Tools, which never ran in
//! Archon, are such a field. Everything else a hand edit can get wrong, such
//! as a broken wire or an unknown formation type, still loads, so validation
//! can name it.

use serde::{Deserialize, Serialize};

use crate::nodes::{Connection, End, Formation, Gate, InputCard, Limit};
use parvi_core::Error;

/// The only mission file schema Parvi reads.
pub const MISSION_SCHEMA: i64 = 1;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Mission {
    pub schema: i64,
    pub id: String,
    pub slug: String,
    pub title: String,
    pub rev: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated_by: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
    /// A mission has one Input card; a file holding several still loads, and
    /// validation names it (`several_input_cards`).
    #[serde(rename = "inputCard")]
    pub input_cards: Vec<InputCard>,
    #[serde(rename = "formation")]
    pub formations: Vec<Formation>,
    #[serde(rename = "gate")]
    pub gates: Vec<Gate>,
    #[serde(rename = "end")]
    pub ends: Vec<End>,
    #[serde(rename = "limit")]
    pub limits: Vec<Limit>,
    #[serde(rename = "connection")]
    pub connections: Vec<Connection>,
}

impl Mission {
    /// Reads a mission file's text.
    pub fn parse(text: &str) -> Result<Mission, Error> {
        let mission: Mission = toml::from_str(text)
            .map_err(|err| Error::new("invalid_definition_source", err.to_string()))?;
        if mission.schema != MISSION_SCHEMA {
            return Err(Error::new(
                "unsupported_schema",
                format!(
                    "mission schema {}; Parvi reads schema {MISSION_SCHEMA}",
                    mission.schema
                ),
            ));
        }
        Ok(mission)
    }

    /// The mission's Input card, the first when a file holds several.
    pub fn input_card(&self) -> Option<&InputCard> {
        self.input_cards.first()
    }

    pub fn formation(&self, id: &str) -> Option<&Formation> {
        self.formations.iter().find(|formation| formation.id == id)
    }

    pub fn gate(&self, id: &str) -> Option<&Gate> {
        self.gates.iter().find(|gate| gate.id == id)
    }

    pub fn end(&self, id: &str) -> Option<&End> {
        self.ends.iter().find(|end| end.id == id)
    }

    pub fn has_input_card(&self, id: &str) -> bool {
        self.input_cards.iter().any(|card| card.id == id)
    }

    /// The title of the node with this id, or "" when it has none.
    pub fn node_title(&self, id: &str) -> &str {
        let titles = (self.formations.iter().map(|n| (&n.id, &n.title)))
            .chain(self.gates.iter().map(|n| (&n.id, &n.title)))
            .chain(self.input_cards.iter().map(|n| (&n.id, &n.title)))
            .chain(self.ends.iter().map(|n| (&n.id, &n.title)))
            .chain(self.limits.iter().map(|n| (&n.id, &n.title)));
        for (node, title) in titles {
            if node == id {
                return title;
            }
        }
        ""
    }

    /// Names a node for the operator: its title, or its id when it has none.
    pub fn node_name<'a>(&'a self, id: &'a str) -> &'a str {
        match self.node_title(id).trim() {
            "" => id,
            title => title,
        }
    }
}

/// One mission file as the listing shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MissionListing {
    pub slug: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<i64>,
    /// Why the file cannot be read, such as a symlink whose target moved;
    /// the other fields are then empty but the slug.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broken: Option<String>,
}
