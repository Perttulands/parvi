//! Materialize text-port refs without blocking on a special file.
use crate::Formation;
use serde_json::{Map, Value};
use std::{
    fs::OpenOptions,
    io::{self, Read},
    path::Path,
};

pub(crate) fn materialize(
    formation: &Formation,
    outputs: &mut Map<String, Value>,
) -> io::Result<()> {
    for port in formation
        .outputs
        .iter()
        .filter(|port| port.kind() == "text")
    {
        let Some(payload) = outputs.get_mut(&port.id).and_then(Value::as_object_mut) else {
            continue;
        };
        let value = payload
            .get("ref")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if value.is_empty() {
            continue;
        }
        let path = Path::new(value);
        if !path.is_absolute() || value.contains("://") || value.contains('\0') {
            return Err(io::Error::other(format!("invalid_output_ref: {value:?}")));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options
            .open(path)
            .map_err(|e| io::Error::other(format!("unavailable_output_ref: {value}: {e}")))?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other(format!(
                "invalid_output_ref: {value} is not a regular file"
            )));
        }
        let mut bytes = Vec::new();
        file.take(8193)
            .read_to_end(&mut bytes)
            .map_err(|e| io::Error::other(format!("unavailable_output_ref: {value}: {e}")))?;
        if bytes.len() > 8192 {
            return Err(io::Error::other(format!("oversized_output_ref: {value}")));
        }
        if bytes.contains(&0) {
            return Err(io::Error::other(format!(
                "invalid_output_ref: {value} is not text"
            )));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| io::Error::other(format!("invalid_output_ref: {value} is not UTF-8")))?;
        payload.insert("text".into(), Value::String(text));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Port;
    use serde_json::json;
    #[test]
    fn text_refs_are_bounded_and_materialized_exactly() {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let root =
            std::env::temp_dir().join(format!("parvi-102-ref-{:x}", u64::from_ne_bytes(random)));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("output");
        std::fs::write(&path, "one\ntwo").unwrap();
        let formation = Formation {
            outputs: vec![Port {
                id: "out".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut payloads = json!({"out":{"ref":path,"text":"summary"}})
            .as_object()
            .unwrap()
            .clone();
        materialize(&formation, &mut payloads).unwrap();
        assert_eq!(payloads["out"]["text"], "one\ntwo");
        std::fs::write(&path, vec![b'x'; 8193]).unwrap();
        assert!(
            materialize(&formation, &mut payloads)
                .unwrap_err()
                .to_string()
                .contains("oversized_output_ref")
        );
        payloads.get_mut("out").unwrap()["ref"] = json!("relative");
        assert!(
            materialize(&formation, &mut payloads)
                .unwrap_err()
                .to_string()
                .contains("invalid_output_ref")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
