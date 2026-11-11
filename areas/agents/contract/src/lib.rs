//! The Agents channel (0x80–0x8f, `docs/protocol.md`, Agents): each node's
//! report of its harnesses, tools, skills checkouts and skills, and the
//! reading of it the page shows (`ui/src/agents/drift.ts`, line for line).
//! Plain data; nothing here draws or sends.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FAILED: &str = "agents_failed";

pub const WATCH: u8 = 0x80;
pub const SYNC: u8 = 0x81;
pub const REPORT: u8 = 0x88;
pub const SYNCED: u8 = 0x89;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Setting {
    pub path: String,
    pub present: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<String>,
    pub signed_in: Option<bool>,
    #[serde(default)]
    pub credentials_present: bool,
    #[serde(default)]
    pub settings: Vec<Setting>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Checkout {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    pub fetch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch_error: Option<String>,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    #[serde(default)]
    pub dirty: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Location {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
    #[serde(default)]
    pub behind: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Skill {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
    #[serde(default)]
    pub behind: u32,
    /// Its state in each skills folder, by folder id.
    #[serde(default)]
    pub links: BTreeMap<String, String>,
    #[serde(default)]
    pub locations: BTreeMap<String, Location>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Folder {
    pub id: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Report {
    #[serde(default)]
    pub home: String,
    #[serde(default)]
    pub folders: Vec<Folder>,
    #[serde(default)]
    pub tools: Vec<Tool>,
    #[serde(default)]
    pub checkouts: Vec<Checkout>,
    #[serde(default)]
    pub skills: Vec<Skill>,
}

/// The role library's HTTP wire data.
pub mod roles {
    use serde::{Deserialize, Serialize};

    pub const PATH: &str = "/api/roles";

    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(default, rename_all = "camelCase")]
    pub struct Role {
        pub schema: u32,
        pub id: String,
        pub display_name: String,
        pub kind: String,
        pub summary: String,
        pub tags: Vec<String>,
        pub status: String,
        pub etag: String,
        pub notes: Vec<Note>,
        pub preset: bool,
    }
    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(default)]
    pub struct Note {
        pub ts: String,
        pub actor: String,
        pub text: String,
    }
    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(default, rename_all = "camelCase")]
    pub struct Roles {
        pub roles: Vec<Role>,
        pub unreadable: Vec<Unreadable>,
        pub harnesses: Vec<Harness>,
        pub effort_policy: Vec<EffortPolicy>,
    }
    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(default)]
    pub struct Harness {
        pub id: String,
        pub efforts: Vec<String>,
        pub models: Vec<serde_json::Value>,
    }
    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(default)]
    pub struct EffortPolicy {
        pub effort: String,
        #[serde(rename = "use")]
        pub usage: String,
    }
    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(default, rename_all = "camelCase")]
    pub struct Use {
        pub mission_id: String,
        pub mission_slug: String,
        pub mission_title: String,
        pub formation_id: String,
        pub formation_title: String,
        pub slot_id: String,
        pub slot_label: String,
    }
    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(default)]
    pub struct Usage {
        pub usage: Vec<Use>,
        pub complete: bool,
        pub unreadable: Vec<Unreadable>,
    }
    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(default)]
    pub struct Unreadable {
        pub name: String,
        pub reason: String,
    }
}
