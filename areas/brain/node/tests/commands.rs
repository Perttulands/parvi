use parvi_node_kit::cli;
use parvi_testkit::{fresh_dir, node::Node};
use serde_json::json;
use std::path::Path;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

async fn run(node: &Node, args: &[&str]) -> (i32, String) {
    let mut full = vec!["--node".to_string(), format!("{}/", node.url())];
    full.extend(args.iter().map(|s| s.to_string()));
    let mut out = Vec::new();
    let status = cli::run("brain", &full, &mut out).await.unwrap();
    (status, String::from_utf8(out).unwrap())
}

#[tokio::test]
async fn brain_commands_save_and_read_changes() {
    assert_eq!(
        parvi_brain::LISTING.commands,
        &[
            "brain_search",
            "brain_read",
            "brain_save",
            "brain_collections",
            "brain_changes"
        ]
    );
    let root = fresh_dir("brain-commands-corpus");
    std::fs::create_dir(root.join("notes")).unwrap();
    std::fs::create_dir(root.join("other")).unwrap();
    std::fs::write(root.join("notes/page.md"), "# First\n").unwrap();
    std::fs::write(root.join("other/older.md"), "# Older\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Fixture"]);
    git(&root, &["config", "user.email", "fixture@example.org"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "Initial"]);
    let base = git(&root, &["rev-parse", "HEAD"]);
    let config = format!("[brain]\ncorpus = {:?}\n", root.to_str().unwrap());
    let node = Node::start(&[parvi_brain::LISTING], &config).await.unwrap();
    let (_, help) = run(&node, &[]).await;
    for verb in ["search", "read", "save", "collections", "changes"] {
        assert!(help.contains(&format!("parvi brain {verb}")), "{help}");
    }
    let missing = node
        .call("brain_search", json!({"query":"north"}))
        .await
        .unwrap_err();
    assert!(missing.message.contains("[brain].citadel"));
    let (status, output) = run(
        &node,
        &[
            "save",
            "notes/page.md",
            "--text",
            "# Updated\n",
            "--summary",
            "Shelve update",
        ],
    )
    .await;
    assert_eq!(status, 0, "{output}");
    assert!(output.contains("Shelve update"), "{output}");
    assert_eq!(
        std::fs::read_to_string(root.join("notes/page.md")).unwrap(),
        "# Updated\n"
    );
    assert_eq!(git(&root, &["rev-list", "--count", "HEAD"]), "2");
    assert_eq!(git(&root, &["log", "-1", "--format=%s"]), "Shelve update");
    let (status, _) = run(
        &node,
        &[
            "save",
            "notes/page.md",
            "--text",
            "# Stale\n",
            "--base",
            &base,
        ],
    )
    .await;
    assert_eq!(status, 1);
    let conflict = node
        .call(
            "brain_save",
            json!({"path":"notes/page.md", "text":"# Stale\n", "base":base}),
        )
        .await
        .unwrap_err();
    assert_eq!(conflict.code, "conflict");
    assert_eq!(
        std::fs::read_to_string(root.join("notes/page.md")).unwrap(),
        "# Updated\n"
    );
    let (status, collections) = run(&node, &["collections"]).await;
    assert_eq!(status, 0);
    assert!(
        collections.contains("name=notes") && collections.contains("pages=1"),
        "{collections}"
    );
    assert!(collections.contains("name=other"), "{collections}");
    let (status, changes) = run(&node, &["changes", "--limit", "2"]).await;
    assert_eq!(status, 0);
    assert!(
        changes.find("notes/page.md").unwrap() < changes.find("other/older.md").unwrap(),
        "{changes}"
    );
    assert_eq!(
        node.call("brain_changes", json!({"limit": 0}))
            .await
            .unwrap_err()
            .code,
        "bad_request"
    );
    node.stop().await;
    std::fs::remove_dir_all(root).unwrap();
}
