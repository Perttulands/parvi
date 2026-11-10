//! The scheduled area on a node with terminals over a real tmux on a
//! scratch socket: its commands over MCP and the CLI, and its HTTP route.
//! Linux and macOS only; skips without tmux unless `PARVI_REQUIRE_TMUX=1`.
#![cfg(unix)]

use axum::http::Method;
use parvi_node_kit::{Build, NodeClient, cli};
use parvi_testkit::node::Node;
use parvi_testkit::tmux::{Server, capture, eventually, pane_text};
use serde_json::{Value, json};

const AREAS: &[(&str, Build)] = &[
    (parvi_terminals::NAME, parvi_terminals::build),
    (parvi_scheduled::NAME, parvi_scheduled::build),
];

/// A node with terminals and scheduled over a scratch tmux holding `alpha`.
async fn start(name: &str, extra: &str) -> Option<(Node, Server)> {
    let tmux = Server::start_in(name)?;
    tmux.new_session("alpha", 100, 30);
    let config = format!("[terminals]\nsocket = {:?}\n{extra}", tmux.socket_path());
    let node = Node::start(AREAS, &config).await.unwrap();
    let engine = node.sessions().engine();
    eventually("alpha's pane read", || {
        pane_text(&engine, "alpha")?.contains('$').then_some(())
    })
    .await;
    Some((node, tmux))
}

#[tokio::test(flavor = "multi_thread")]
async fn schedules_a_prompt_runs_it_now_and_manages_it() {
    let Some((node, tmux)) = start("scheduled", "").await else {
        return;
    };
    let made = node
        .call(
            "scheduled_create",
            json!({ "name": "nudge", "prompt": "echo nudged-$((6*7))",
                    "sessions": ["alpha"], "every": 60 }),
        )
        .await
        .unwrap();
    let id = made["task"]["id"].as_str().unwrap().to_owned();
    let listed = node.call("scheduled_list", json!({})).await.unwrap();
    assert_eq!(listed["tasks"][0]["sessions"], json!(["alpha"]), "{listed}");

    node.call("scheduled_run", json!({ "id": id }))
        .await
        .unwrap();
    eventually("the prompt ran", || {
        capture(&tmux, "alpha")
            .lines()
            .any(|line| line.trim() == "nudged-42")
            .then_some(())
    })
    .await;

    let edited = node
        .call(
            "scheduled_edit",
            json!({ "id": id, "cron": "0 9 * * 1", "timezone": "UTC" }),
        )
        .await
        .unwrap();
    assert_eq!(edited["task"]["schedule"]["type"], "cron");
    let paused = node
        .call("scheduled_pause", json!({ "id": id }))
        .await
        .unwrap();
    assert_eq!(paused["task"]["paused"], true);
    let copy = node
        .call("scheduled_copy", json!({ "id": id }))
        .await
        .unwrap();
    assert_eq!(copy["task"]["name"], "nudge (copy)");
    assert_eq!(copy["task"]["paused"], true);
    node.call("scheduled_delete", json!({ "id": id }))
        .await
        .unwrap();
    let gone = node.call("scheduled_show", json!({ "id": id })).await;
    assert!(gone.unwrap_err().message.contains("not found"));
    let both = node
        .call(
            "scheduled_create",
            json!({ "name": "x", "prompt": "y", "sessions": ["alpha"], "every": 5, "cron": "* * * * *" }),
        )
        .await;
    assert!(both.unwrap_err().message.contains("not both"));
    node.stop().await;
}

/// A task made through the CLI is the one the HTTP route lists, and the
/// CLI offers the verbs the old binary had.
#[tokio::test(flavor = "multi_thread")]
async fn the_cli_and_the_http_route_see_the_same_tasks() {
    let Some((node, _tmux)) = start("scheduled-cli", "").await else {
        return;
    };
    let url = format!("{}/", node.url());
    let run = |args: &[&str]| {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let url = url.clone();
        async move {
            let mut out = Vec::new();
            let mut full = args;
            full.extend(["--node".into(), url]);
            let code = cli::run("scheduled", &full, &mut out).await.unwrap();
            (code, String::from_utf8(out).unwrap())
        }
    };
    let (_, verbs) = run(&[]).await;
    for verb in [
        "list", "show", "create", "edit", "copy", "pause", "resume", "run", "delete",
    ] {
        assert!(
            verbs.contains(&format!("parvi scheduled {verb}")),
            "{verb}: {verbs}"
        );
    }
    let (code, made) = run(&[
        "create",
        "--name",
        "cli",
        "--prompt",
        "hello",
        "--sessions",
        "alpha",
        "--every",
        "30",
        "--json",
    ])
    .await;
    assert_eq!(code, 0, "{made}");
    let made: Value = serde_json::from_str(&made).unwrap();
    let id = made["task"]["id"].as_str().unwrap();

    let client = NodeClient::new(Some(&url)).unwrap();
    let (tasks, _) = client
        .request(Method::GET, "/api/scheduled-tasks", None, None)
        .await
        .unwrap();
    assert_eq!(tasks["success"], true);
    assert_eq!(tasks["data"]["tasks"][0]["id"], id);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_with_scheduled_turned_off_has_no_commands_or_route() {
    let Some((node, _tmux)) = start("scheduled-off", "[scheduled]\nenabled = false\n").await else {
        return;
    };
    let url = format!("{}/", node.url());
    let client = NodeClient::new(Some(&url)).unwrap();
    let (health, _) = client
        .request(Method::GET, "/api/health", None, None)
        .await
        .unwrap();
    assert_eq!(health["off"], json!(["scheduled"]));
    let refused = client
        .request(Method::GET, "/api/scheduled-tasks", None, None)
        .await
        .unwrap_err();
    assert!(format!("{refused:#}").contains("404"), "{refused:#}");
    let args: Vec<String> = ["list", "--node", &url].map(String::from).to_vec();
    assert!(cli::run("scheduled", &args, &mut Vec::new()).await.is_err());
    node.stop().await;
}
