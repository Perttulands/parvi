//! The Agents commands and channel on an isolated node and scratch home.
use std::time::Duration;

use axum::http::Method;
use futures_util::{SinkExt, StreamExt};
use parvi_agents_contract::{FAILED, REPORT, SYNC, SYNCED, WATCH};
use parvi_core::Error;
use parvi_node_kit::{NodeClient, cli};
use parvi_testkit::{fresh_dir, node::Node, sessions};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test(flavor = "multi_thread")]
async fn commands_and_channel_work_over_the_node() {
    let home = fresh_dir("parvi-agents-node-home");
    let skill = home.join(".agents/skills/example/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(
        &skill,
        "---\nname: example\ndescription: A fixture skill.\n---\n",
    )
    .unwrap();
    let config = format!("[agents]\nhome = {:?}\npath = []\n", home);
    let node = Node::start(&[sessions::LISTING, parvi_agents::LISTING], &config)
        .await
        .unwrap();
    let client = NodeClient::new(Some(&node.url())).unwrap();

    let (health, _) = client
        .request(Method::GET, "/api/health", None, None)
        .await
        .unwrap();
    assert_eq!(health["status"], "ok");
    assert!(
        !health["off"]
            .as_array()
            .unwrap()
            .iter()
            .any(|area| area == "agents")
    );
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"tools/list", "params":{}});
    let (list, _) = client
        .request(Method::POST, "/mcp", Some(request), None)
        .await
        .unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .filter(|name| !parvi_node_kit::own::COMMANDS.contains(name))
        .collect();
    assert_eq!(
        names,
        [
            "agents_show",
            "agents_sync",
            "agents_link",
            "role_list",
            "role_inspect",
            "role_usage",
            "role_new",
            "role_edit",
            "role_retire",
            "role_restore",
            "role_delete",
            "role_characters"
        ]
    );
    assert_eq!(parvi_agents::LISTING.commands, names);

    let args = vec!["--node".into(), node.url()];
    let mut output = Vec::new();
    assert_eq!(cli::run("agents", &args, &mut output).await.unwrap(), 0);
    let output = String::from_utf8(output).unwrap();
    assert!(
        output.contains("show") && output.contains("sync") && output.contains("link"),
        "{output}"
    );
    let report = node.call("agents_show", json!({})).await.unwrap();
    assert_eq!(report["node"], node.id());
    assert_eq!(report["skills"][0]["name"], "example");
    let error = node
        .call("agents_sync", json!({"checkout":"~/not-a-checkout"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, FAILED);
    assert!(error.message.contains("not a checkout"));
    let error = node
        .call("agents_link", json!({"checkout":"~/not-a-checkout"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, FAILED);
    assert!(error.message.contains("not a checkout"));
    let remote = node
        .call("agents_show", json!({"node":"another-node"}))
        .await
        .unwrap_err();
    assert_eq!(remote.code, "unknown_node");

    let (mut ws, _) = tokio_tungstenite::connect_async(node.ws()).await.unwrap();
    ws.send(Message::Binary(vec![WATCH, 1].into()))
        .await
        .unwrap();
    let frame = next_kind(&mut ws, REPORT).await;
    let report: Value = serde_json::from_slice(&frame[1..]).unwrap();
    assert_eq!(report["skills"][0]["name"], "example");
    ws.send(Message::Binary(
        [&[SYNC][..], &42_u32.to_le_bytes(), b"~/not-a-checkout"]
            .concat()
            .into(),
    ))
    .await
    .unwrap();
    let frame = next_kind(&mut ws, SYNCED).await;
    assert_eq!(&frame[1..6], &[42, 0, 0, 0, 0]);
    let error: Error = serde_json::from_slice(&frame[6..]).unwrap();
    assert_eq!(error.code, FAILED);
    assert!(error.message.contains("not a checkout"));
    ws.send(Message::Binary(vec![WATCH, 0].into()))
        .await
        .unwrap();
    drop(ws);
    node.stop().await;
    std::fs::remove_dir_all(home).unwrap();
}

async fn next_kind(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    kind: u8,
) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = ws.next().await {
            if let Message::Binary(bytes) = message.unwrap()
                && bytes.first() == Some(&kind)
            {
                return bytes.to_vec();
            }
        }
        panic!("socket closed before frame {kind:#x}")
    })
    .await
    .expect("timed out waiting for frame")
}

#[tokio::test(flavor = "multi_thread")]
async fn role_routes_and_commands_keep_versions_and_usage() {
    let root = fresh_dir("parvi-roles-node");
    let config = format!("[agents]\nhome = {:?}\npath = []\n", root);
    let node = Node::start(&[sessions::LISTING, parvi_agents::LISTING], &config)
        .await
        .unwrap();
    let client = NodeClient::new(Some(&node.url())).unwrap();
    let (created, tag) = client.request(Method::POST, "/api/roles", Some(json!({"id":"writer","displayName":"Writer","kind":"specialist","summary":"Writes","capabilities":["draft"],"personality":"calm"})), None).await.unwrap();
    assert_eq!(created["displayName"], "Writer");
    assert!(!tag.is_empty());
    let (list, _) = client
        .request(
            Method::GET,
            "/api/roles?capable=draft&assignable=true",
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(list["roles"][0]["id"], "writer");
    let (read, etag) = client
        .request(Method::GET, "/api/roles/writer", None, None)
        .await
        .unwrap();
    assert_eq!(etag, tag);
    assert_eq!(read["etag"], tag);
    let (changed, next) = client
        .request(
            Method::PATCH,
            "/api/roles/writer",
            Some(json!({"summary":"Revised","note":"Keep context","capabilities":["draft"]})),
            Some(&tag),
        )
        .await
        .unwrap();
    assert_eq!(changed["summary"], "Revised");
    assert_eq!(changed["notes"][0]["text"], "Keep context");
    assert!(
        changed["tags"]
            .as_array()
            .unwrap()
            .contains(&json!("personality:calm"))
    );
    let conflict = client
        .request(
            Method::PATCH,
            "/api/roles/writer",
            Some(json!({"summary":"stale"})),
            Some(&tag),
        )
        .await
        .unwrap_err();
    assert_eq!(
        conflict
            .downcast_ref::<parvi_node_kit::NodeError>()
            .unwrap()
            .code,
        "conflict"
    );
    let (usage, _) = client
        .request(Method::GET, "/api/roles/writer/usage", None, None)
        .await
        .unwrap();
    assert_eq!(usage["complete"], true);
    let (retired, next) = client
        .request(
            Method::PATCH,
            "/api/roles/writer",
            Some(json!({"retire":true})),
            Some(&next),
        )
        .await
        .unwrap();
    assert_eq!(retired["status"], "retired");
    let (filtered, _) = client
        .request(
            Method::GET,
            "/api/roles?capable=draft&assignable=true",
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(filtered["roles"], json!([]));
    let (restored, next) = client
        .request(
            Method::PATCH,
            "/api/roles/writer",
            Some(json!({"retire":false})),
            Some(&next),
        )
        .await
        .unwrap();
    assert_eq!(restored["status"], "active");
    let (deleted, _) = client
        .request(Method::DELETE, "/api/roles/writer", None, Some(&next))
        .await
        .unwrap();
    assert_eq!(deleted["deleted"], "writer");
    assert!(!root.join("roles/writer.toml").exists());
    let names = [
        "role_list",
        "role_inspect",
        "role_usage",
        "role_new",
        "role_edit",
        "role_retire",
        "role_restore",
        "role_delete",
        "role_characters",
    ];
    assert_eq!(&parvi_agents::LISTING.commands[3..], names);
    let role = node
        .call("role_new", json!({"role":"editor","display_name":"Editor"}))
        .await
        .unwrap();
    assert_eq!(role["id"], "editor");
    assert_eq!(
        node.call("role_inspect", json!({"role":"editor"}))
            .await
            .unwrap()["id"],
        "editor"
    );
    node.stop().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn watching_does_not_fetch_and_sync_failures_survive_refresh_and_other_watchers() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let home = fresh_dir("agents-no-fetch-watch");
    let bin = home.join("bin");
    let links = home.join(".agents/skills");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&links).unwrap();
    let git = bin.join("git");
    std::fs::write(&git, "#!/bin/sh\nif [ \"$3\" = fetch ]; then printf '%s\\n' \"$2\" >> \"$HOME/fetch-log\"; case \"$2\" in */first) if [ -f \"$HOME/block-fetch\" ]; then : > \"$HOME/fetch-started\"; while [ ! -f \"$HOME/continue-fetch\" ]; do :; done; fi;; esac; echo 'fatal: credentials unavailable' >&2; exit 1; fi\nexit 0\n").unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["first", "second"] {
        let checkout = home.join(name);
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        let skill = checkout.join("skills").join(name);
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Fixture\n---\n"),
        )
        .unwrap();
        symlink(&skill, links.join(name)).unwrap();
    }
    let config = format!("[agents]\nhome = {:?}\npath = [{:?}]\n", home, bin);
    let node = Node::start(&[sessions::LISTING, parvi_agents::LISTING], &config)
        .await
        .unwrap();
    let (mut first, _) = tokio_tungstenite::connect_async(node.ws()).await.unwrap();
    let (mut second, _) = tokio_tungstenite::connect_async(node.ws()).await.unwrap();
    for ws in [&mut first, &mut second] {
        ws.send(Message::Binary(vec![WATCH, 1].into()))
            .await
            .unwrap();
        let report: Value = serde_json::from_slice(&next_kind(ws, REPORT).await[1..]).unwrap();
        assert_eq!(report["checkouts"].as_array().unwrap().len(), 2);
        assert!(
            report["checkouts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["fetch"] == "local")
        );
    }
    let show = node.call("agents_show", json!({})).await.unwrap();
    assert!(
        show["checkouts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["fetch"] == "local")
    );
    assert!(
        !home.join("fetch-log").exists(),
        "reading or watching must not fetch"
    );
    first
        .send(Message::Binary(
            [&[SYNC][..], &17_u32.to_le_bytes(), b"~/first"]
                .concat()
                .into(),
        ))
        .await
        .unwrap();
    let answer = next_kind(&mut first, SYNCED).await;
    assert_eq!(answer[5], 0);
    let error: Error = serde_json::from_slice(&answer[6..]).unwrap();
    assert_eq!(error.code, "agents_fetch_failed");
    assert!(error.message.contains("credentials unavailable"));
    let other = next_report_with_failure(&mut second, "first", 1).await;
    assert_eq!(
        other["checkouts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "first")
            .unwrap()["fetchError"],
        "fatal: credentials unavailable"
    );
    let error = node
        .call("agents_sync", json!({"checkout":"~/second"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "agents_fetch_failed");
    let both = next_report_with_failure(&mut second, "second", 2).await;
    assert_eq!(
        both["checkouts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["fetch"] == "failed")
            .count(),
        2
    );
    first
        .send(Message::Binary(vec![WATCH, 0].into()))
        .await
        .unwrap();
    first
        .send(Message::Binary(vec![WATCH, 1].into()))
        .await
        .unwrap();
    let refreshed = next_report_with_failure(&mut first, "second", 2).await;
    assert_eq!(
        refreshed["checkouts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["fetch"] == "failed")
            .count(),
        2
    );
    // A second Sync can arrive while a first is in flight, and the first
    // watcher can disconnect without losing either checkout's result.
    std::fs::write(home.join("block-fetch"), "").unwrap();
    first
        .send(Message::Binary(
            [&[SYNC][..], &18_u32.to_le_bytes(), b"~/first"]
                .concat()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !home.join("fetch-started").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first fetch started");
    second
        .send(Message::Binary(
            [&[SYNC][..], &19_u32.to_le_bytes(), b"~/second"]
                .concat()
                .into(),
        ))
        .await
        .unwrap();
    drop(first);
    std::fs::write(home.join("continue-fetch"), "").unwrap();
    let answer = next_kind(&mut second, SYNCED).await;
    assert_eq!(&answer[1..5], &19_u32.to_le_bytes());
    assert_eq!(answer[5], 0);
    let after_disconnect = node.call("agents_show", json!({})).await.unwrap();
    assert_eq!(
        after_disconnect["checkouts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["fetch"] == "failed")
            .count(),
        2
    );
    let log = std::fs::read_to_string(home.join("fetch-log")).unwrap();
    assert_eq!(
        log.lines().count(),
        4,
        "only explicit Sync commands fetch: {log}"
    );
    drop(second);
    node.stop().await;
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
async fn next_report_with_failure(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    name: &str,
    count: usize,
) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = next_kind(ws, REPORT).await;
            let report: Value = serde_json::from_slice(&frame[1..]).unwrap();
            let checkouts = report["checkouts"].as_array().unwrap();
            if checkouts
                .iter()
                .any(|c| c["name"] == name && c["fetch"] == "failed")
                && checkouts.iter().filter(|c| c["fetch"] == "failed").count() == count
            {
                return report;
            }
        }
    })
    .await
    .expect("report with retained sync failures")
}
