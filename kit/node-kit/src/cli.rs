//! `parvi <noun> <verb> [args]`: every command of the node's areas. The
//! command line is a client of the node's MCP tools: it reads `tools/list`
//! for the tool `<noun>_<verb>` and its arguments, and calls it through
//! `tools/call`, so the CLI and MCP cannot drift apart. Required arguments
//! may be given in order; every argument is also `--name value`.

use anyhow::{Context, Result, bail};
use axum::http::Method;
use parvi_core::Error;
use serde_json::{Map, Value, json};
use std::io::Write;

use crate::client::NodeClient;

/// Runs one command, writing its answer to `out`. The exit code is 1 when
/// the tool failed. `node`, when given, is the node to talk to; else the
/// one this machine runs, or `--node` among `args`.
pub async fn run(noun: &str, args: &[String], out: &mut impl Write) -> Result<i32> {
    let (node, args) = take_node(args)?;
    // A URL is the node the command talks to; an id is the node the tool
    // runs on, reached through the one it talks to.
    let (url, on) = match node {
        Some(node) if node.contains("://") => (Some(node), None),
        on => (None, on),
    };
    let client = NodeClient::new(url.as_deref())?;
    let tools = list(&client).await?;
    let ours: Vec<&Value> = tools
        .iter()
        .filter(|t| {
            name(t)
                .strip_prefix(noun)
                .is_some_and(|v| v.starts_with('_'))
        })
        .collect();
    if ours.is_empty() {
        // An area the node turned off has no commands; say which are off.
        let off = match client.request(Method::GET, "/api/health", None, None).await {
            Ok((health, _)) => health["off"]
                .as_array()
                .map(|off| {
                    off.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
            Err(_) => String::new(),
        };
        let off = match off.as_str() {
            "" => String::new(),
            off => format!("; turned off on this node: {off}"),
        };
        bail!(
            "no command {noun}; commands: {}{off}",
            nouns(&tools).join(", ")
        );
    }
    let Some(verb) = args.first().filter(|v| !v.starts_with('-')) else {
        writeln!(out, "{}", verbs(noun, &ours))?;
        return Ok(0);
    };
    let tool_name = format!("{noun}_{}", verb.replace('-', "_"));
    let Some(tool) = ours.iter().find(|t| name(t) == tool_name) else {
        bail!("no command {noun} {verb}\n{}", verbs(noun, &ours));
    };
    let rest = &args[1..];
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        writeln!(out, "{}", help(noun, verb, tool))?;
        return Ok(0);
    }
    let json_out = rest.iter().any(|a| a == "--json" || a == "--json=true");
    let rest: Vec<String> = rest
        .iter()
        .filter(|a| *a != "--json" && *a != "--json=true")
        .cloned()
        .collect();
    let mut arguments = parse(&tool["inputSchema"], &rest)
        .with_context(|| format!("usage: {}", usage(noun, verb, tool)))?;
    if let Some(on) = on {
        arguments["node"] = Value::String(on);
    }
    let answer = call(&client, &tool_name, arguments).await?;
    let failed = answer["isError"].as_bool().unwrap_or(false);
    let text = answer["content"][0]["text"].as_str().unwrap_or("");
    if failed {
        match serde_json::from_str::<Error>(text) {
            Ok(error) if json_out => eprintln!("{}", json!({ "error": error })),
            Ok(error) => eprintln!("parvi {noun} {verb}: {error}"),
            Err(_) => eprintln!("parvi {noun} {verb}: {text}"),
        }
        return Ok(1);
    }
    let value: Value = serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.into()));
    if json_out {
        writeln!(out, "{}", serde_json::to_string_pretty(&value)?)?;
    } else {
        write!(out, "{}", render(&value))?;
    }
    Ok(0)
}

/// The commands, one line each, for `parvi help`.
pub async fn overview(node: Option<&str>) -> Result<String> {
    let tools = list(&NodeClient::new(node)?).await?;
    let mut lines = Vec::new();
    for t in &tools {
        let (noun, verb) = name(t).split_once('_').unwrap_or((name(t), ""));
        lines.push(format!(
            "  parvi {noun} {}  {}",
            verb.replace('_', "-"),
            first_line(t)
        ));
    }
    Ok(lines.join("\n"))
}

fn take_node(args: &[String]) -> Result<(Option<String>, Vec<String>)> {
    let mut node = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--node" {
            node = Some(it.next().context("--node needs a node id or URL")?.clone());
        } else if let Some(v) = a.strip_prefix("--node=") {
            node = Some(v.to_owned());
        } else {
            rest.push(a.clone());
        }
    }
    Ok((node, rest))
}

async fn rpc(client: &NodeClient, method: &str, params: Value) -> Result<Value> {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let (answer, _) = client
        .request(Method::POST, "/mcp", Some(body), None)
        .await
        .context("is the Parvi node running? (parvi serve)")?;
    if let Some(error) = answer.get("error") {
        bail!(
            "{}",
            error["message"].as_str().unwrap_or("the node refused")
        );
    }
    Ok(answer["result"].clone())
}

async fn list(client: &NodeClient) -> Result<Vec<Value>> {
    let result = rpc(client, "tools/list", json!({})).await?;
    Ok(result["tools"].as_array().cloned().unwrap_or_default())
}

async fn call(client: &NodeClient, tool: &str, arguments: Value) -> Result<Value> {
    rpc(
        client,
        "tools/call",
        json!({"name": tool, "arguments": arguments}),
    )
    .await
}

fn name(tool: &Value) -> &str {
    tool["name"].as_str().unwrap_or("")
}

fn first_line(tool: &Value) -> &str {
    tool["description"]
        .as_str()
        .unwrap_or("")
        .split(". ")
        .next()
        .unwrap_or("")
}

fn nouns(tools: &[Value]) -> Vec<String> {
    let mut nouns: Vec<String> = tools
        .iter()
        .filter_map(|t| name(t).split_once('_').map(|(n, _)| n.to_owned()))
        .collect();
    nouns.dedup();
    nouns
}

fn verbs(noun: &str, tools: &[&Value]) -> String {
    tools
        .iter()
        .map(|t| {
            let verb = name(t)[noun.len() + 1..].replace('_', "-");
            format!("  {}\n      {}", usage(noun, &verb, t), first_line(t))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `parvi noun verb <required>... [--optional value]...`
fn usage(noun: &str, verb: &str, tool: &Value) -> String {
    let schema = &tool["inputSchema"];
    let mut line = format!("parvi {noun} {verb}");
    for r in required(schema) {
        line.push_str(&format!(" <{r}>"));
    }
    for (key, spec) in properties(schema) {
        if required(schema).contains(&key.as_str()) {
            continue;
        }
        let flag = key.replace('_', "-");
        match spec["type"].as_str() {
            Some("boolean") => line.push_str(&format!(" [--{flag}|--no-{flag}]")),
            _ => line.push_str(&format!(" [--{flag} {key}]")),
        }
    }
    line + " [--json] [--node id|url]"
}

fn help(noun: &str, verb: &str, tool: &Value) -> String {
    let mut text = format!(
        "usage: {}\n{}\n",
        usage(noun, verb, tool),
        tool["description"].as_str().unwrap_or("")
    );
    for (key, spec) in properties(&tool["inputSchema"]) {
        let flag = key.replace('_', "-");
        let mut line = format!("\n  --{flag}  {}", spec["type"].as_str().unwrap_or("value"));
        if let Some(d) = spec["description"].as_str() {
            line.push_str(&format!(": {d}"));
        }
        if let Some(values) = spec["enum"].as_array() {
            let values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
            line.push_str(&format!(" (one of {})", values.join(", ")));
        }
        if let Some(d) = spec.get("default") {
            line.push_str(&format!(" (default {d})"));
        }
        text.push_str(&line);
    }
    text
}

/// The tool's arguments but `node`, which is the command line's `--node`.
fn properties(schema: &Value) -> Vec<(String, Value)> {
    schema["properties"]
        .as_object()
        .map(|p| {
            p.iter()
                .filter(|(k, _)| *k != "node")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn required(schema: &Value) -> Vec<&str> {
    schema["required"]
        .as_array()
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// The tool's arguments from the command line, typed by its schema.
fn parse(schema: &Value, args: &[String]) -> Result<Value> {
    let props = properties(schema);
    let spec = |key: &str| props.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    let mut out = Map::new();
    let mut positional = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let Some(flag) = a.strip_prefix("--") else {
            positional.push(a.clone());
            continue;
        };
        let (key, inline) = match flag.split_once('=') {
            Some((k, v)) => (k.to_owned(), Some(v.to_owned())),
            None => (flag.to_owned(), None),
        };
        let key = key.replace('-', "_");
        if let Some(negated) = key.strip_prefix("no_")
            && spec(negated).is_some_and(|s| s["type"] == "boolean")
            && inline.is_none()
        {
            out.insert(negated.to_owned(), Value::Bool(false));
            continue;
        }
        let s = spec(&key).with_context(|| format!("unknown flag --{key}"))?;
        let raw = match (inline, s["type"].as_str()) {
            (Some(v), _) => v,
            (None, Some("boolean")) => "true".into(),
            (None, _) => it
                .next()
                .with_context(|| format!("--{key} needs a value"))?
                .clone(),
        };
        let value = typed(&key, &s, &raw)?;
        match (s["type"].as_str(), out.get_mut(&key)) {
            (Some("array"), Some(Value::Array(items))) => {
                items.extend(value.as_array().cloned().unwrap_or_default())
            }
            _ => {
                out.insert(key, value);
            }
        }
    }
    let mut waiting = required(schema)
        .into_iter()
        .filter(|r| !out.contains_key(*r))
        .map(str::to_owned)
        .collect::<Vec<_>>()
        .into_iter()
        .peekable();
    // The last required argument, if it is a list, takes every value left.
    let mut list: Option<String> = None;
    for p in positional {
        if let Some(key) = &list {
            if let Some(Value::Array(items)) = out.get_mut(key) {
                items.push(Value::String(p));
            }
            continue;
        }
        let key = waiting
            .next()
            .with_context(|| format!("unexpected argument {p:?}"))?;
        let s = spec(&key).unwrap_or(Value::Null);
        if s["type"] == "array" && waiting.peek().is_none() {
            list = Some(key.clone());
        }
        out.insert(key.clone(), typed(&key, &s, &p)?);
    }
    if let Some(missing) = waiting.next() {
        bail!("missing <{missing}>");
    }
    Ok(Value::Object(out))
}

fn typed(key: &str, spec: &Value, raw: &str) -> Result<Value> {
    Ok(match spec["type"].as_str() {
        Some("boolean") => Value::Bool(match raw {
            "true" | "yes" | "1" => true,
            "false" | "no" | "0" => false,
            _ => bail!("--{key} is true or false"),
        }),
        Some("integer") => json!(
            raw.parse::<i64>()
                .with_context(|| format!("--{key} is a whole number"))?
        ),
        Some("number") => json!(
            raw.parse::<f64>()
                .with_context(|| format!("--{key} is a number"))?
        ),
        Some("array") => json!([raw]),
        Some("object") => serde_json::from_str(raw).with_context(|| format!("--{key} is JSON"))?,
        _ => Value::String(raw.to_owned()),
    })
}

/// A readable answer: a list as one row per item, an object as `key: value`
/// lines, text as itself. `--json` prints the value as it is.
fn render(value: &Value) -> String {
    match value {
        Value::String(s) => format!("{}\n", s.trim_end()),
        Value::Array(items) if items.iter().all(Value::is_object) && !items.is_empty() => {
            let mut text = String::new();
            for item in items {
                text.push_str(&row(item));
                text.push('\n');
            }
            text
        }
        Value::Array(items) => items.iter().map(scalar).collect::<Vec<_>>().join("\n") + "\n",
        Value::Object(map) => {
            let mut text = String::new();
            for (k, v) in map {
                match v {
                    Value::Array(items)
                        if items.iter().all(Value::is_object) && !items.is_empty() =>
                    {
                        text.push_str(&format!("{k}:\n"));
                        for item in items {
                            text.push_str(&format!("  {}\n", row(item)));
                        }
                    }
                    _ => text.push_str(&format!("{k}: {}\n", scalar(v))),
                }
            }
            text
        }
        other => format!("{}\n", scalar(other)),
    }
}

/// One object on one line: its plain fields, tab-separated as `key=value`.
fn row(item: &Value) -> String {
    item.as_object()
        .map(|m| {
            m.iter()
                .filter(|(_, v)| !v.is_object() && !v.is_array() && !v.is_null())
                .map(|(k, v)| format!("{k}={}", scalar(v)))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .unwrap_or_default()
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "session": {"type": "string"},
                "text": {"type": "string"},
                "enter": {"type": "boolean", "default": true},
                "pane": {"type": "integer"},
            },
            "required": ["session", "text"],
        })
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn required_arguments_go_in_order_and_flags_are_typed() {
        let v = parse(
            &schema(),
            &args(&["main", "hello there", "--pane", "2", "--no-enter"]),
        )
        .unwrap();
        assert_eq!(
            v,
            json!({"session": "main", "text": "hello there", "pane": 2, "enter": false})
        );
        let v = parse(&schema(), &args(&["--text=hi", "main", "--enter"])).unwrap();
        assert_eq!(v, json!({"session": "main", "text": "hi", "enter": true}));
    }

    #[test]
    fn kebab_and_underscore_flags_resolve_to_schema_keys() {
        let schema = json!({"properties":{"to_node":{"type":"string"},"display_name":{"type":"string"},"context_paths":{"type":"array"},"copy_on_select":{"type":"boolean"}}});
        assert_eq!(
            parse(
                &schema,
                &args(&[
                    "--to-node",
                    "n",
                    "--display-name=x",
                    "--context-paths",
                    "a",
                    "--context_paths",
                    "b",
                    "--no-copy-on-select"
                ])
            )
            .unwrap(),
            json!({"to_node":"n","display_name":"x","context_paths":["a","b"],"copy_on_select":false})
        );
    }

    #[test]
    fn a_last_required_list_takes_every_value_left() {
        let keys = json!({
            "type": "object",
            "properties": {
                "session": {"type": "string"},
                "keys": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["session", "keys"],
        });
        let v = parse(&keys, &args(&["main", "echo hi", "Enter"])).unwrap();
        assert_eq!(v, json!({"session": "main", "keys": ["echo hi", "Enter"]}));
        let v = parse(&keys, &args(&["--keys", "C-c", "main"])).unwrap();
        assert_eq!(v, json!({"session": "main", "keys": ["C-c"]}));
    }

    #[test]
    fn wrong_or_missing_arguments_are_refused() {
        assert!(parse(&schema(), &args(&["main"])).is_err());
        assert!(parse(&schema(), &args(&["main", "hi", "extra"])).is_err());
        assert!(parse(&schema(), &args(&["main", "hi", "--colour", "red"])).is_err());
        assert!(parse(&schema(), &args(&["main", "hi", "--pane", "two"])).is_err());
    }

    #[test]
    fn lists_read_as_one_row_per_item() {
        let text = render(
            &json!([{"id": "%1", "name": "main", "panes": [1]}, {"id": "%2", "name": "build"}]),
        );
        assert_eq!(text, "id=%1\tname=main\nid=%2\tname=build\n");
    }
}
