//! Reading a command's JSON arguments and describing them: the few helpers
//! every area's commands share. A failure is an `anyhow` error whose message
//! names the argument; the area decides which error code it travels under.

use anyhow::{Context, Result};
use serde_json::{Value, json};

/// A required string argument.
pub fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{key} is required, as a string"))
}

/// An optional string argument; `null` counts as absent.
pub fn maybe_text<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .with_context(|| format!("{key} is a string")),
    }
}

/// A boolean argument, `default` if absent.
pub fn flag(args: &Value, key: &str, default: bool) -> Result<bool> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_bool()
            .with_context(|| format!("{key} is true or false")),
    }
}

/// A JSON Schema object with these properties, `required` among them.
pub fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_read_by_type_and_failures_name_the_argument() {
        let args = json!({ "name": "x", "note": null, "on": true, "n": 3 });
        assert_eq!(text(&args, "name").unwrap(), "x");
        assert!(
            text(&args, "n")
                .unwrap_err()
                .to_string()
                .contains("n is required")
        );
        assert!(text(&args, "missing").is_err());
        assert_eq!(maybe_text(&args, "note").unwrap(), None);
        assert_eq!(maybe_text(&args, "missing").unwrap(), None);
        assert_eq!(maybe_text(&args, "name").unwrap(), Some("x"));
        assert!(
            maybe_text(&args, "n")
                .unwrap_err()
                .to_string()
                .contains("n is a string")
        );
        assert!(flag(&args, "on", false).unwrap());
        assert!(flag(&args, "missing", true).unwrap());
        assert!(!flag(&args, "note", false).unwrap());
        assert!(flag(&args, "name", false).is_err());
    }

    #[test]
    fn an_object_schema_lists_its_properties_and_required() {
        let schema = object(json!({ "id": { "type": "string" } }), &["id"]);
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["id"]));
        assert_eq!(schema["properties"]["id"]["type"], "string");
    }
}
