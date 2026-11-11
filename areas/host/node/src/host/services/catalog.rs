//! The service catalog: which services this node shows and how each is
//! made (CHROTE's controller catalog, `services.json`, as it is). Read
//! again on every list and action, so an edit counts at once.

use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub read_only: bool,
    pub components: Vec<Component>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum Component {
    /// A unit of a user's systemd manager.
    #[serde(rename = "systemd-user")]
    User { user: String, unit: String },
    /// A system unit: shown, never started or stopped.
    #[serde(rename = "systemd-system")]
    System { unit: String },
    /// Up when anything answers at the loopback URL.
    #[serde(rename = "http")]
    Http { url: String },
}

#[derive(Deserialize)]
struct File {
    services: Vec<Entry>,
}

/// Reads and checks the catalog, in its order. Every error is a sentence.
pub fn read(path: &Path) -> Result<Vec<Entry>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("The service catalog {} does not read: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("The service catalog {}: {e}", path.display()))
}

pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    let file: File = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut seen = std::collections::HashSet::new();
    for e in &file.services {
        if !ident(&e.id) || !seen.insert(e.id.clone()) {
            return Err(format!("invalid or duplicate service id {:?}", e.id));
        }
        if e.name.is_empty() || e.components.is_empty() {
            return Err(format!("{} needs a name and components", e.id));
        }
        for c in &e.components {
            match c {
                Component::User { unit, user } => {
                    if !unit_name(unit) || user.is_empty() {
                        return Err(format!("{}: invalid unit {unit:?}", e.id));
                    }
                }
                Component::System { unit } => {
                    if !e.read_only || !unit_name(unit) {
                        return Err(format!("{}: system units must be read-only", e.id));
                    }
                }
                Component::Http { url } => {
                    if !e.read_only || !url.starts_with("http://127.0.0.1:") {
                        return Err(format!(
                            "{}: HTTP probes must be read-only loopback URLs",
                            e.id
                        ));
                    }
                }
            }
        }
    }
    Ok(file.services)
}

/// `^[a-z0-9][a-z0-9_-]*$`
pub fn ident(id: &str) -> bool {
    let mut chars = id.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// `^[A-Za-z0-9_.@:-]+\.service$`
fn unit_name(unit: &str) -> bool {
    unit.strip_suffix(".service").is_some_and(|stem| {
        !stem.is_empty()
            && stem
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.@:-".contains(c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_chrotes_catalog() {
        let text = r#"{"services":[
            {"id":"desk","name":"Desk","link":"http://x:1/","components":[
                {"kind":"systemd-user","user":"me","unit":"desk@example.service"}]},
            {"id":"room","name":"Room","readOnly":true,"components":[
                {"kind":"http","url":"http://127.0.0.1:8080/"},
                {"kind":"systemd-system","unit":"sample.service"}]}]}"#;
        let entries = parse(text).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[1].read_only);
        assert_eq!(entries[0].description, "");
    }

    #[test]
    fn refuses_what_the_controller_refuses() {
        let one = |c: &str, ro: bool| {
            format!(
                r#"{{"services":[{{"id":"a","name":"A","readOnly":{ro},"components":[{c}]}}]}}"#
            )
        };
        let user = r#"{"kind":"systemd-user","user":"me","unit":"a.service"}"#;
        assert!(parse(&one(user, false)).is_ok());
        let system = r#"{"kind":"systemd-system","unit":"a.service"}"#;
        assert!(
            parse(&one(system, false)).is_err(),
            "a system unit must be read-only"
        );
        let remote = r#"{"kind":"http","url":"http://example.com/"}"#;
        assert!(
            parse(&one(remote, true)).is_err(),
            "probes stay on loopback"
        );
        let bad_unit = r#"{"kind":"systemd-user","user":"me","unit":"a;rm.service"}"#;
        assert!(parse(&one(bad_unit, false)).is_err());
        let dup = format!(
            r#"{{"services":[{{"id":"a","name":"A","components":[{user}]}},{{"id":"a","name":"B","components":[{user}]}}]}}"#
        );
        assert!(parse(&dup).is_err());
        assert!(!ident("Upper") && !ident("-a") && ident("vsk-zone_2"));
    }
}
