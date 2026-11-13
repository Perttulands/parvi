//! Agents against a scratch home: a skills checkout with an origin, links
//! into it from both skills folders, stand-in harness commands, and a
//! credentials file whose content must never come out. Sync runs on a
//! scratch tmux server. Linux and macOS only; the sync test skips without
//! tmux unless `PARVI_REQUIRE_TMUX=1`.
#![cfg(unix)]

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use parvi_agents::{Fetch, Home, gather};

const SECRET: &str = "sk-82-do-not-show";

/// A scratch home, removed when the test ends.
struct Scratch {
    root: PathBuf,
    home: Home,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn executable(path: &Path, script: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn skill(dir: &Path, name: &str, description: &str) {
    write(
        &dir.join("skills").join(name).join("SKILL.md"),
        &format!("---\nname: {name}\ndescription: {description}\n---\n\nBody.\n"),
    );
}

/// The home: `kit` cloned from `origin.git`, which then gains a commit
/// touching `beta` that the clone has not fetched.
fn scratch() -> Scratch {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "parvi-82-agents-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("home");
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&root, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
    git(&work, &["init", "-q", "-b", "main"]);
    skill(&work, "alpha", "First skill.");
    skill(&work, "beta", "Second skill.");
    skill(&work, "delta", "Not linked anywhere.");
    executable(
        &work.join("install.sh"),
        "#!/bin/sh\ntouch installed.mark\n",
    );
    write(&work.join(".gitignore"), "installed.mark\n");
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "first"]);
    git(
        &work,
        &[
            "push",
            "-q",
            root.join("origin.git").to_str().unwrap(),
            "main",
        ],
    );
    git(
        &dir.parent().unwrap().join(""),
        &["clone", "-q", "origin.git", "home/kit"],
    );
    skill(&work, "beta", "Second skill, better.");
    git(&work, &["commit", "-q", "-am", "better beta"]);
    git(
        &work,
        &[
            "push",
            "-q",
            root.join("origin.git").to_str().unwrap(),
            "main",
        ],
    );

    let kit = dir.join("kit").join("skills");
    let claude = dir.join(".claude").join("skills");
    let agents = dir.join(".agents").join("skills");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir_all(&agents).unwrap();
    symlink(kit.join("alpha"), claude.join("alpha")).unwrap();
    symlink(kit.join("beta"), claude.join("beta")).unwrap();
    symlink(kit.join("gone"), claude.join("gone")).unwrap();
    symlink(kit.join("alpha"), agents.join("alpha")).unwrap();
    skill(&dir.join(".agents"), "mine", "A copy of my own.");

    let bin = dir.join("bin");
    executable(
        &bin.join("claude"),
        "#!/bin/sh\necho '2.1.0 (Claude Code)'\n",
    );
    executable(&bin.join("codex"), "#!/bin/sh\necho 'codex-cli 0.166.0'\n");
    executable(
        &bin.join("gh"),
        "#!/bin/sh\necho 'gh version 2.100.0 (2026-01-01)'\n",
    );
    write(&dir.join(".claude/.credentials.json"), SECRET);
    write(&dir.join(".claude/settings.json"), "{}");
    write(&dir.join(".codex/auth.json"), "");

    Scratch {
        home: Home {
            dir: dir.clone(),
            path: vec![bin, "/usr/bin".into(), "/bin".into()],
        },
        root,
    }
}

/// The report from its `key` list on.
fn section<'a>(json: &'a str, key: &str) -> &'a str {
    &json[json
        .find(&format!("\"{key}\":["))
        .unwrap_or_else(|| panic!("no {key} in {json}"))..]
}

/// The JSON object in `json` that starts with `{"<key>":"<value>"`.
fn object<'a>(json: &'a str, key: &str, value: &str) -> &'a str {
    let start = json
        .find(&format!("{{\"{key}\":\"{value}\""))
        .unwrap_or_else(|| panic!("no {key} {value} in {json}"));
    let mut depth = 0;
    for (i, c) in json[start..].char_indices() {
        match c {
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return &json[start..=start + i];
                }
            }
            _ => {}
        }
    }
    panic!("unclosed object");
}

#[tokio::test]
async fn reports_tools_skills_and_the_checkout() {
    let scratch = scratch();
    let home = &scratch.home;
    let report = gather(home, Fetch::Fetching).await;
    let json = report.json(home);
    assert!(!json.contains(SECRET), "a secret came out: {json}");

    let claude = object(section(&json, "tools"), "id", "claude");
    assert!(claude.contains("\"version\":\"2.1.0\""), "{claude}");
    assert!(claude.contains("\"path\":\"~/bin/claude\""), "{claude}");
    assert!(
        claude.contains("\"signedIn\":null,\"credentialsPresent\":true"),
        "{claude}"
    );
    assert!(
        claude.contains("{\"path\":\"~/.claude/settings.json\",\"present\":true}"),
        "{claude}"
    );
    assert!(
        claude.contains("{\"path\":\"~/.claude/CLAUDE.md\",\"present\":false}"),
        "{claude}"
    );
    let codex = object(section(&json, "tools"), "id", "codex");
    assert!(
        codex.contains("\"version\":\"0.166.0\"")
            && codex.contains("\"signedIn\":null,\"credentialsPresent\":false"),
        "{codex}"
    );
    let pi = object(section(&json, "tools"), "id", "pi");
    assert!(
        pi.contains("\"command\":\"pi\",\"skills\""),
        "no path or version: {pi}"
    );
    assert!(object(section(&json, "tools"), "id", "gh").contains("\"version\":\"2.100.0\""));

    let links = |name: &str| {
        let skill = object(section(&json, "skills"), "name", name);
        serde_json::from_str::<serde_json::Value>(skill).unwrap()["links"].clone()
    };
    assert_eq!(
        links("alpha"),
        serde_json::json!({"claude":"linked","agents":"linked"})
    );
    assert_eq!(
        links("beta"),
        serde_json::json!({"claude":"linked","agents":"missing"})
    );
    assert_eq!(
        links("gone"),
        serde_json::json!({"claude":"broken","agents":"missing"})
    );
    assert_eq!(
        links("mine"),
        serde_json::json!({"claude":"missing","agents":"copy"})
    );
    assert_eq!(
        links("delta"),
        serde_json::json!({"claude":"missing","agents":"missing"})
    );
    assert!(
        object(section(&json, "skills"), "name", "alpha")
            .contains("\"description\":\"First skill.\"")
    );
    assert!(object(section(&json, "skills"), "name", "alpha").contains("\"checkout\":\"~/kit\""));
    assert!(!object(section(&json, "skills"), "name", "mine").contains("\"checkout\""));

    let kit = object(section(&json, "checkouts"), "name", "kit");
    assert!(
        kit.contains("\"path\":\"~/kit\"") && kit.contains("\"branch\":\"main\""),
        "{kit}"
    );
    assert!(
        kit.contains("\"upstream\":\"origin/main\"") && kit.contains("\"setup\":\"install.sh\""),
        "{kit}"
    );
    assert!(
        kit.contains("\"fetch\":\"fetching\"") && kit.contains("\"behind\":0"),
        "{kit}"
    );
    assert!(
        kit.contains("\"subject\":\"first\"") && kit.contains("\"dirty\":false"),
        "{kit}"
    );

    git(&home.dir.join("kit"), &["fetch", "--quiet"]);
    let report = gather(home, Fetch::Fetched).await;
    let json = report.json(home);
    let kit = object(section(&json, "checkouts"), "name", "kit");
    assert!(
        kit.contains("\"fetch\":\"fetched\"") && kit.contains("\"behind\":1"),
        "{kit}"
    );
    assert!(object(section(&json, "skills"), "name", "beta").contains("\"behind\":1"));
    assert!(object(section(&json, "skills"), "name", "alpha").contains("\"behind\":0"));
}

#[tokio::test]
async fn a_failed_fetch_says_why_and_keeps_the_last_counts() {
    let scratch = scratch();
    let home = &scratch.home;
    git(
        &home.dir.join("kit"),
        &["remote", "set-url", "origin", "/nonexistent/82.git"],
    );
    let report = gather(home, Fetch::Failed("fetch failed".into())).await;
    let json = report.json(home);
    let kit = object(section(&json, "checkouts"), "name", "kit");
    assert!(
        kit.contains("\"fetch\":\"failed\",\"fetchError\":"),
        "{kit}"
    );
    assert!(kit.contains("\"behind\":0"), "{kit}");
}

#[tokio::test]
async fn sync_fetches_fast_forwards_and_runs_setup_in_a_session() {
    use parvi_testkit::node::Node;
    use parvi_testkit::tmux::{Server, eventually};
    let Some(tmux) = Server::start_in("agents-sync") else {
        assert!(
            std::env::var_os("PARVI_REQUIRE_TMUX").is_none(),
            "tmux is required"
        );
        return;
    };
    tmux.new_session("sync-kit", 100, 30);
    let config = format!("[terminals]\nsocket = {:?}\n", tmux.socket_path());
    let node = Node::start(&[parvi_terminals::LISTING, parvi_agents::LISTING], &config)
        .await
        .unwrap();
    let sessions = node.sessions();
    eventually("the first session", || {
        sessions
            .list()
            .iter()
            .any(|s| s.name == "sync-kit")
            .then_some(())
    })
    .await;
    let scratch = scratch();
    let home = &scratch.home;
    assert!(
        parvi_agents::sync(home, sessions.as_ref(), "/tmp")
            .await
            .is_err()
    );
    let session = parvi_agents::sync(home, sessions.as_ref(), "~/kit")
        .await
        .unwrap();
    assert_eq!(session, "sync-kit-2");
    let kit = home.dir.join("kit");
    eventually("the setup to run", || {
        kit.join("installed.mark").exists().then_some(())
    })
    .await;
    let origin = git(&scratch.root.join("origin.git"), &["rev-parse", "main"]);
    assert_eq!(git(&kit, &["rev-parse", "HEAD"]), origin);
    node.stop().await;
}

#[test]
fn commands_match_the_old_agents_tools() {
    assert_eq!(
        parvi_agents::LISTING
            .commands
            .iter()
            .copied()
            .filter(|n| n.starts_with("agents_"))
            .collect::<Vec<_>>(),
        &["agents_show", "agents_sync", "agents_link"]
    );
}
