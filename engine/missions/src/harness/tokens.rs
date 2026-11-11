use super::{
    adapter::{Pointer, TokenUsage},
    transcript::{pointer_matches, records, s, text},
};
use parvi_term::waiting::harness::Harness;
use serde_json::Value;
use std::{collections::HashMap, fs, io, path::Path};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
fn num(v: &Value, key: &str) -> i64 {
    v[key].as_i64().unwrap_or_default()
}
fn usage(v: &Value, pi: bool) -> TokenUsage {
    if pi {
        TokenUsage {
            input: num(v, "input"),
            cache_write: num(v, "cacheWrite"),
            cache_read: num(v, "cacheRead"),
            output: num(v, "output"),
        }
    } else {
        TokenUsage {
            input: num(v, "input_tokens"),
            cache_write: num(v, "cache_creation_input_tokens"),
            cache_read: num(v, "cache_read_input_tokens"),
            output: num(v, "output_tokens"),
        }
    }
}
fn claude_file(
    path: &Path,
    pointer: Option<&str>,
    since: Option<OffsetDateTime>,
) -> io::Result<(TokenUsage, Option<OffsetDateTime>)> {
    let mut messages = HashMap::new();
    let mut counting = pointer.is_none();
    let mut started = None;
    for r in records(path)? {
        let at = OffsetDateTime::parse(s(&r, "timestamp"), &Rfc3339).ok();
        let msg = &r["message"];
        if pointer.is_some_and(|p| {
            s(&r, "type") == "user"
                && r["isMeta"] != true
                && pointer_matches(&text(&msg["content"]), p)
        }) {
            messages.clear();
            counting = true;
            started = at;
            continue;
        }
        if !counting
            || s(&r, "type") != "assistant"
            || msg["usage"].is_null()
            || s(msg, "id").is_empty()
        {
            continue;
        }
        if let Some(since) = since
            && at.is_none_or(|at| at < since)
        {
            continue;
        }
        messages.insert(s(msg, "id").to_owned(), usage(&msg["usage"], false));
    }
    let mut total = TokenUsage::default();
    for value in messages.into_values() {
        total.add(value);
    }
    Ok((total, started))
}
fn claude(path: &Path, pointer: &str) -> io::Result<TokenUsage> {
    let (mut total, since) = claude_file(path, Some(pointer), None)?;
    if let Some(since) = since {
        let dir = path.with_extension("").join("subagents");
        match fs::read_dir(dir) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry?.path();
                    if path.extension().is_some_and(|s| s == "jsonl") {
                        total.add(claude_file(&path, None, Some(since))?.0);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}
fn codex(path: &Path, pointer: &str) -> io::Result<TokenUsage> {
    let mut before = TokenUsage::default();
    let mut latest = TokenUsage::default();
    let mut consumed = false;
    for r in records(path)? {
        let p = &r["payload"];
        if s(&r, "type") == "response_item"
            && s(p, "type") == "message"
            && s(p, "role") == "user"
            && text(&p["content"]) == pointer
        {
            consumed = true;
            before = latest;
        }
        if s(&r, "type") == "event_msg" && s(p, "type") == "token_count" && !p["info"].is_null() {
            let t = &p["info"]["total_token_usage"];
            let cache_read = num(t, "cached_input_tokens");
            let cache_write = num(t, "cache_write_input_tokens");
            latest = TokenUsage {
                input: num(t, "input_tokens") - cache_read - cache_write,
                cache_write,
                cache_read,
                output: num(t, "output_tokens"),
            };
        }
    }
    if !consumed {
        return Ok(TokenUsage::default());
    }
    Ok(TokenUsage {
        input: latest.input - before.input,
        cache_write: latest.cache_write - before.cache_write,
        cache_read: latest.cache_read - before.cache_read,
        output: latest.output - before.output,
    })
}
fn pi(path: &Path, pointer: &str) -> io::Result<TokenUsage> {
    let mut total = TokenUsage::default();
    let mut counting = false;
    for r in records(path)? {
        let msg = &r["message"];
        if s(&r, "type") == "message"
            && s(msg, "role") == "user"
            && text(&msg["content"]) == pointer
        {
            counting = true;
            total = TokenUsage::default();
            continue;
        }
        let u = if s(&r, "type") == "message" && s(msg, "role") == "assistant" {
            &msg["usage"]
        } else {
            &r["usage"]
        };
        if counting && !u.is_null() {
            total.add(usage(u, true));
        }
    }
    Ok(total)
}
pub(super) fn read(h: Harness, pointer: &Pointer) -> io::Result<TokenUsage> {
    let path = pointer.transcript.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "transcript path has not been discovered",
        )
    })?;
    match h {
        Harness::Claude => claude(path, &pointer.text),
        Harness::Codex => codex(path, &pointer.text),
        Harness::Pi => pi(path, &pointer.text),
    }
}
