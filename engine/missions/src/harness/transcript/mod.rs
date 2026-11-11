mod claude;
mod codex;
mod pi;

use super::adapter::{Pointer, TranscriptRoots, Turn, TurnState};
use claude::read_claude;
use codex::read_codex;
use parvi_term::waiting::harness::Harness;
use pi::read_pi;
use serde_json::Value;
use std::{
    fs,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
};

pub(super) fn text(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_owned();
    }
    value
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| {
                    b["type"] == "text" || b["type"] == "input_text" || b["type"] == "output_text"
                })
                .filter_map(|b| b["text"].as_str())
                .collect()
        })
        .unwrap_or_default()
}
pub(super) fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
pub(super) fn pointer_matches(actual: &str, expected: &str) -> bool {
    if actual == expected {
        return true;
    }
    let Some(rest) = actual.strip_prefix("\n\n<pasted_content id=\"") else {
        return false;
    };
    let Some((id, body)) = rest.split_once("\">\n") else {
        return false;
    };
    !id.is_empty()
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        && body == format!("{expected}\n</pasted_content id=\"{id}\">\n")
}
pub(super) fn records(path: &Path) -> io::Result<Vec<Value>> {
    let mut input = BufReader::new(fs::File::open(path)?);
    let mut line = Vec::new();
    let mut result = Vec::new();
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // The harness may be halfway through writing a JSON record, including
        // halfway through a UTF-8 character. Retry it on the next wakeup.
        let Ok(record) = serde_json::from_slice(&line) else {
            break;
        };
        result.push(record);
    }
    Ok(result)
}
fn matches_sentinel(text: &str, sentinel: &str) -> bool {
    if sentinel.is_empty() {
        return false;
    }
    text.split(crate::contract_names::DONE)
        .skip(1)
        .filter_map(|s| s.split_once(">>>"))
        .any(|(fields, _)| {
            fields
                .split_whitespace()
                .filter_map(|part| part.split_once('='))
                .any(|(k, v)| k == "run-id" && v.trim_matches('"') == sentinel)
        })
}
fn native_pointer(h: Harness, v: &Value, pointer: &str) -> bool {
    match h {
        Harness::Claude => {
            s(v, "type") == "user" && pointer_matches(&text(&v["message"]["content"]), pointer)
        }
        Harness::Codex => {
            s(v, "type") == "response_item"
                && s(&v["payload"], "type") == "message"
                && s(&v["payload"], "role") == "user"
                && text(&v["payload"]["content"]) == pointer
        }
        Harness::Pi => {
            s(v, "type") == "message"
                && s(&v["message"], "role") == "user"
                && text(&v["message"]["content"]) == pointer
        }
    }
}
fn changed(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn cwd_matches(a: &str, b: &Path) -> bool {
    Path::new(a) == b
        || fs::canonicalize(a)
            .ok()
            .zip(fs::canonicalize(b).ok())
            .is_some_and(|(a, b)| a == b)
}
fn walk(root: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            walk(&entry.path(), out)?;
        } else if ty.is_file() && entry.path().extension().is_some_and(|s| s == "jsonl") {
            out.push(entry.path());
        }
    }
    Ok(())
}
fn verify_settings(h: Harness, pointer: &Pointer, turn: &Turn) -> io::Result<()> {
    let binding = pointer
        .binding
        .as_ref()
        .ok_or_else(|| changed("dispatch model and effort are required to verify completion"))?;
    let expected = binding.model.to_lowercase();
    let observed = turn.model.to_lowercase();
    let model_ok = expected.is_empty()
        || match h {
            Harness::Claude => {
                expected == observed
                    || ["opus", "sonnet", "haiku"].contains(&expected.as_str())
                        && observed.starts_with(&format!("claude-{expected}-"))
            }
            Harness::Codex => expected == observed,
            Harness::Pi => {
                if let Some((provider, model)) = expected.split_once('/') {
                    observed
                        .split_once('/')
                        .is_some_and(|(seen_provider, seen_model)| {
                            provider == seen_provider && seen_model.contains(model)
                        })
                } else {
                    observed.contains(&expected)
                }
            }
        };
    let effort = if binding.effort.is_empty() {
        "medium"
    } else {
        &binding.effort
    };
    let effort_ok = h != Harness::Codex && turn.effort.is_empty() || turn.effort == effort;
    if !model_ok || !effort_ok {
        return Err(changed("seat model or effort does not match the dispatch"));
    }
    Ok(())
}
pub(super) fn find(
    h: Harness,
    roots: &TranscriptRoots,
    pointer: &Pointer,
    sentinel: &str,
) -> io::Result<Turn> {
    let mut paths = Vec::new();
    if let Some(path) = &pointer.transcript {
        paths.push(path.clone());
    } else {
        walk(
            match h {
                Harness::Claude => &roots.claude,
                Harness::Codex => &roots.codex,
                Harness::Pi => &roots.pi,
            },
            &mut paths,
        )?;
    }
    let mut selected: Option<Turn> = None;
    for path in paths {
        if pointer.transcript.is_none() {
            if let Some(since) = pointer.not_before
                && let Ok(modified) = fs::metadata(&path).and_then(|metadata| metadata.modified())
                && modified < since
            {
                continue;
            }
            if matches!(h, Harness::Codex | Harness::Pi) {
                let mut line = String::new();
                let mut file = BufReader::new(fs::File::open(&path)?);
                file.read_line(&mut line)?;
                let Ok(head) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let cwd = if h == Harness::Codex {
                    s(&head["payload"], "cwd")
                } else {
                    s(&head, "cwd")
                };
                if !cwd_matches(cwd, &pointer.cwd) {
                    continue;
                }
            }
        }
        let rows = records(&path)?;
        let mut result = match h {
            Harness::Claude => read_claude(&rows, pointer, sentinel),
            Harness::Codex => read_codex(&rows, pointer, sentinel),
            Harness::Pi => read_pi(&rows, pointer, sentinel),
        }?;
        if !result.consumed {
            continue;
        }
        if pointer
            .session_id
            .as_ref()
            .is_some_and(|id| *id != result.session_id)
        {
            return Err(changed("native session identity changed"));
        }
        if matches!(result.state, TurnState::Completed { .. }) {
            verify_settings(h, pointer, &result)?;
        }
        if selected.is_some() {
            return Err(changed(
                "multiple native sessions consumed the dispatched pointer",
            ));
        }
        result.transcript = Some(path);
        selected = Some(result);
    }
    Ok(selected.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_requires_a_parvi_marker_for_the_run() {
        assert!(matches_sentinel(
            "<<<PARVI-DONE run-id=run_1 status=ok>>>",
            "run_1"
        ));
        assert!(!matches_sentinel(
            "<<<ARCHON-DONE run-id=run_1 status=ok>>>",
            "run_1"
        ));
        assert!(!matches_sentinel("<<<ARBITRARY>>>", "<<<ARBITRARY>>>"));
    }
    #[test]
    fn partial_utf8_record_is_left_for_the_next_read() {
        let path =
            std::env::temp_dir().join(format!("parvi-104-partial-{}.jsonl", std::process::id()));
        fs::write(&path, b"{\"type\":\"first\"}\n{\"text\":\"\xf0\x9f").unwrap();
        let rows = records(&path).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(s(&rows[0], "type"), "first");
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn pasted_pointer_accepts_only_the_native_wrapper() {
        assert!(pointer_matches(
            "\n\n<pasted_content id=\"57da\">\nbrief\n</pasted_content id=\"57da\">\n",
            "brief"
        ));
        assert!(!pointer_matches(
            "\n\n<pasted_content id=\"57da\">\nbrief\n</pasted_content id=\"9999\">\n",
            "brief"
        ));
        assert!(!pointer_matches("quoted brief", "brief"));
    }
}
