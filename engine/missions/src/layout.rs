//! A mission's layout: where its cards and notes sit on the canvas and which
//! lane each wire takes. It is a view of the mission, kept beside it as
//! `<slug>.layout.toml`; a mission without one is arranged when shown.

use serde::{Deserialize, Serialize};

use crate::Error;

pub const LAYOUT_SCHEMA: i64 = 1;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Layout {
    pub schema: i64,
    pub mission_id: String,
    /// The mission revision the layout was last saved against.
    pub mission_rev: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
    #[serde(rename = "node")]
    pub nodes: Vec<LayoutNode>,
    #[serde(rename = "edge")]
    pub edges: Vec<LayoutEdge>,
}

/// A card's place: a node id, or `note:<target>` for a dragged sticky.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutNode {
    pub id: String,
    pub x: i64,
    pub y: i64,
}

/// The lane a wire, by connection id, takes.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutEdge {
    pub id: String,
    pub lane: String,
}

impl Layout {
    pub fn parse(text: &str) -> Result<Layout, Error> {
        let layout: Layout = toml::from_str(text)
            .map_err(|err| Error::new("invalid_definition_source", err.to_string()))?;
        if layout.schema != LAYOUT_SCHEMA {
            return Err(Error::new(
                "unsupported_schema",
                format!("layout schema {}", layout.schema),
            ));
        }
        Ok(layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_reads_places_and_lanes_strictly() {
        let text = "schema = 1\nmissionId = \"msn_a\"\nmissionRev = 3\n\n[[node]]\nid = \"fmn_a\"\nx = 40\ny = -20\n\n[[edge]]\nid = \"edge_a\"\nlane = \"top\"\n";
        let layout = Layout::parse(text).unwrap();
        assert_eq!(
            (layout.nodes[0].y, layout.edges[0].lane.as_str()),
            (-20, "top")
        );
        let repeated = text.replace("x = 40", "x = 40\nx = 41");
        assert_eq!(
            Layout::parse(&repeated).unwrap_err().code,
            "invalid_definition_source"
        );
    }
}
