#[allow(dead_code)]
mod run_support;
use parvi_mission_model::RunStart;
use parvi_missions::{
    Coordinator, ExecutionSink, Executor, FormationExecution, FormationResult, LabExecutor,
    RunSettings,
};
use run_support::{Scratch, gated_mission};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn setup(root: &Scratch) -> RunSettings {
    let checkout = root.0.join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    git(&checkout, &["init", "-b", "main"]);
    git(&checkout, &["config", "user.email", "test@example.invalid"]);
    git(&checkout, &["config", "user.name", "Test"]);
    std::fs::write(checkout.join("base"), "base").unwrap();
    git(&checkout, &["add", "base"]);
    git(&checkout, &["commit", "-m", "base"]);
    RunSettings {
        repositories: BTreeMap::from([("owner/repo".into(), checkout)]),
        ..Default::default()
    }
}
fn request() -> RunStart {
    RunStart {
        mission: "hello".into(),
        input_card_id: "inp_hello".into(),
        expected_rev: 1,
        repository: "owner/repo".into(),
        inputs: BTreeMap::from([("brief".into(), "hello".into())]),
        ..Default::default()
    }
}
fn wait(c: &Coordinator, id: &str) -> parvi_mission_model::RunProjection {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let p = c.run(id).unwrap();
        if p.r#final || p.status == "blocked" {
            return p;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn turns(root: &Scratch, id: &str) -> Vec<String> {
    std::fs::read_dir(root.0.join("worktrees"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|s| s.starts_with(&format!(".turn-{id}-")))
        .collect()
}
struct Committer {
    dirty: bool,
}
impl Executor for Committer {
    fn execute_formation(
        &self,
        req: FormationExecution,
        sink: ExecutionSink,
    ) -> Result<FormationResult, parvi_missions::Error> {
        {
            std::fs::write(req.cwd.join("agent-file"), "committed by agent").unwrap();
            git(&req.cwd, &["add", "agent-file"]);
            git(&req.cwd, &["commit", "-m", "agent commit"]);
            git(
                &req.cwd,
                &["commit", "--allow-empty", "-m", "agent checkpoint"],
            );
            if self.dirty {
                std::fs::write(req.cwd.join("dirty-file"), "uncommitted").unwrap();
            }
            LabExecutor
                .execute_formation(req, sink)
                .map_err(|error| error.message)
        }
        .map_err(|message: String| parvi_missions::Error::new("executor_error", message))
    }
}
#[test]
fn publishes_agent_commits_with_clean_and_dirty_trees() {
    for dirty in [false, true] {
        let root = Scratch::new();
        let settings = setup(&root);
        let c = Coordinator::open(&root.0, settings, Box::new(Committer { dirty })).unwrap();
        let id = c.start_mission(request()).unwrap();
        let p = wait(&c, &id);
        assert_eq!(p.status, "succeeded");
        let cwd = root.0.join("worktrees").join(&id);
        assert_eq!(
            std::fs::read_to_string(cwd.join("agent-file")).unwrap(),
            "committed by agent"
        );
        if dirty {
            assert_eq!(
                std::fs::read_to_string(cwd.join("dirty-file")).unwrap(),
                "uncommitted"
            );
        }
        assert!(
            git(&cwd, &["log", "--format=%s"])
                .lines()
                .any(|s| s == "agent commit")
        );
        assert!(
            git(&cwd, &["log", "--format=%s"])
                .lines()
                .any(|s| s == "agent checkpoint")
        );
        assert!(turns(&root, &id).is_empty());
    }
}
struct Failure {
    mode: &'static str,
    started: Arc<AtomicBool>,
}
impl Executor for Failure {
    fn execute_formation(
        &self,
        req: FormationExecution,
        sink: ExecutionSink,
    ) -> Result<FormationResult, parvi_missions::Error> {
        (|| -> Result<FormationResult, String> {
            std::fs::write(req.cwd.join("unfinished"), "partial").unwrap();
            self.started.store(true, Ordering::Release);
            if self.mode == "error" {
                return Err("executor failed".into());
            }
            if self.mode == "cancel" {
                while !req.canceled.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                return Err("canceled".into());
            }
            let id = req.run_id.clone();
            let mut result = LabExecutor
                .execute_formation(req, sink)
                .map_err(|error| error.message)?;
            result.text = format!("<<<PARVI-ESCALATE run-id={id} severity=stop reason=blocked>>>");
            Ok(result)
        })()
        .map_err(|message: String| parvi_missions::Error::new("executor_error", message))
    }
}
#[test]
fn error_cancel_and_escalation_remove_their_turn_worktrees() {
    for mode in ["error", "cancel", "escalate"] {
        let root = Scratch::new();
        let settings = setup(&root);
        let started = Arc::new(AtomicBool::new(false));
        let c = Coordinator::open(
            &root.0,
            settings,
            Box::new(Failure {
                mode,
                started: started.clone(),
            }),
        )
        .unwrap();
        let id = c.start_mission(request()).unwrap();
        if mode == "cancel" {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !started.load(Ordering::Acquire) {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            c.abort(&id, "human:test", "stop").unwrap();
        }
        let p = wait(&c, &id);
        assert_eq!(
            p.status,
            if mode == "cancel" {
                "canceled"
            } else {
                "blocked"
            }
        );
        assert!(
            turns(&root, &id).is_empty(),
            "{mode}: {:?}",
            turns(&root, &id)
        );
    }
}
struct Locking;
impl Executor for Locking {
    fn execute_formation(
        &self,
        req: FormationExecution,
        sink: ExecutionSink,
    ) -> Result<FormationResult, parvi_missions::Error> {
        {
            std::fs::write(req.cwd.join("changed"), "value").unwrap();
            let primary = req.cwd.parent().unwrap().join(&req.run_id);
            let lock = git(&primary, &["rev-parse", "--git-path", "index.lock"]);
            std::fs::write(lock, "locked").unwrap();
            LabExecutor
                .execute_formation(req, sink)
                .map_err(|error| error.message)
        }
        .map_err(|message: String| parvi_missions::Error::new("executor_error", message))
    }
}
#[test]
fn failed_publication_does_not_request_a_human_verdict() {
    let root = Scratch::new();
    let settings = setup(&root);
    std::fs::write(
        root.0.join("missions/hello.mission.toml"),
        gated_mission("kinds = [\"human\"]"),
    )
    .unwrap();
    let c = Coordinator::open(&root.0, settings, Box::new(Locking)).unwrap();
    let id = c.start_mission(request()).unwrap();
    let p = wait(&c, &id);
    assert_eq!(p.status, "blocked");
    assert!(p.waiting_gates.is_empty());
    assert!(!p.events.iter().any(|e| e.kind == "human_input_requested"));
    let primary = root.0.join("worktrees").join(&id);
    let lock = git(&primary, &["rev-parse", "--git-path", "index.lock"]);
    std::fs::remove_file(lock).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match c.resume(&id, Default::default()) {
            Ok(_) => break,
            Err(e) if e.code == "conflict" => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("{e}"),
        }
    }
    loop {
        let p = c.run(&id).unwrap();
        if !p.waiting_gates.is_empty() {
            break;
        }
        assert!(!p.r#final && p.status != "blocked" && Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(turns(&root, &id).is_empty());
}

#[test]
fn recovery_removes_a_turn_left_after_settlement() {
    use sha2::{Digest, Sha256};
    let root = Scratch::new();
    let settings = setup(&root);
    let c = Coordinator::open(
        &root.0,
        settings.clone(),
        Box::new(Committer { dirty: false }),
    )
    .unwrap();
    let id = c.start_mission(request()).unwrap();
    assert_eq!(wait(&c, &id).status, "succeeded");
    drop(c);
    let primary = root.0.join("worktrees").join(&id);
    let turn = root.0.join("worktrees").join(format!(
        ".turn-{id}-{}-1",
        hex::encode(Sha256::digest(b"fmn_work"))
    ));
    git(
        &primary,
        &[
            "worktree",
            "add",
            "--detach",
            turn.to_str().unwrap(),
            "HEAD",
        ],
    );
    let ledger = root.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&ledger).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let settled = lines
        .iter()
        .position(|line| {
            let e: serde_json::Value = serde_json::from_str(line).unwrap();
            e["type"] == "workspace_settled"
        })
        .unwrap();
    let retained: Vec<_> = lines[..=settled]
        .iter()
        .filter(|line| {
            let e: serde_json::Value = serde_json::from_str(line).unwrap();
            e["type"] != "workspace_started"
        })
        .enumerate()
        .map(|(i, line)| {
            let mut e: serde_json::Value = serde_json::from_str(line).unwrap();
            e["seq"] = (i as u64 + 1).into();
            serde_json::to_string(&e).unwrap()
        })
        .collect();
    std::fs::write(&ledger, retained.join("\n") + "\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(
            &root.0,
            settings.clone(),
            Box::new(Committer { dirty: false }),
        ) {
            Ok(c) => break c,
            Err(_) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    assert_eq!(wait(&c, &id).status, "succeeded");
    assert!(!turn.exists());
    assert!(turns(&root, &id).is_empty());
    assert!(
        !git(&primary, &["worktree", "list", "--porcelain"])
            .contains(&turn.to_string_lossy().to_string())
    );
}

#[test]
fn recovery_discards_an_interrupted_attempt_before_retrying() {
    use sha2::{Digest, Sha256};
    let root = Scratch::new();
    let settings = setup(&root);
    let c = Coordinator::open(
        &root.0,
        settings.clone(),
        Box::new(Committer { dirty: false }),
    )
    .unwrap();
    let id = c.start_mission(request()).unwrap();
    assert_eq!(wait(&c, &id).status, "succeeded");
    drop(c);
    let primary = root.0.join("worktrees").join(&id);
    git(&primary, &["reset", "--hard", "HEAD~2"]);
    let turn = root.0.join("worktrees").join(format!(
        ".turn-{id}-{}-1",
        hex::encode(Sha256::digest(b"fmn_work"))
    ));
    git(
        &primary,
        &[
            "worktree",
            "add",
            "--detach",
            turn.to_str().unwrap(),
            "HEAD",
        ],
    );
    std::fs::write(turn.join("partial"), "interrupted").unwrap();
    let ledger = root.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&ledger).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let started = lines
        .iter()
        .position(|line| {
            let e: serde_json::Value = serde_json::from_str(line).unwrap();
            e["type"] == "workspace_started"
        })
        .unwrap();
    std::fs::write(&ledger, lines[..=started].join("\n") + "\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(
            &root.0,
            settings.clone(),
            Box::new(Committer { dirty: false }),
        ) {
            Ok(c) => break c,
            Err(_) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    let result = wait(&c, &id);
    assert_eq!(result.status, "succeeded");
    assert!(
        result
            .events
            .iter()
            .any(|e| e.kind == "node_started" && e.node_id == "fmn_work" && e.attempt == 2)
    );
    assert!(!turn.exists());
    assert!(turns(&root, &id).is_empty());
    assert!(!primary.join("partial").exists());
}

#[test]
fn recovery_finishes_settlement_after_worktree_was_removed() {
    let root = Scratch::new();
    let settings = setup(&root);
    let c = Coordinator::open(
        &root.0,
        settings.clone(),
        Box::new(Committer { dirty: false }),
    )
    .unwrap();
    let id = c.start_mission(request()).unwrap();
    assert_eq!(wait(&c, &id).status, "succeeded");
    drop(c);
    let ledger = root.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&ledger).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let published = lines
        .iter()
        .position(|line| {
            let e: serde_json::Value = serde_json::from_str(line).unwrap();
            e["type"] == "workspace_published"
        })
        .unwrap();
    std::fs::write(&ledger, lines[..=published].join("\n") + "\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(
            &root.0,
            settings.clone(),
            Box::new(Committer { dirty: false }),
        ) {
            Ok(c) => break c,
            Err(_) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    let p = wait(&c, &id);
    assert_eq!(p.status, "succeeded");
    assert!(p.events.iter().any(|e| e.kind == "workspace_settled"));
    assert!(turns(&root, &id).is_empty());
}

#[test]
fn recovery_does_not_duplicate_a_published_prefix_of_agent_commits() {
    use sha2::{Digest, Sha256};
    for event_kind in ["workspace_snapshot", "workspace_commit"] {
        let root = Scratch::new();
        let settings = setup(&root);
        let c = Coordinator::open(
            &root.0,
            settings.clone(),
            Box::new(Committer { dirty: true }),
        )
        .unwrap();
        let id = c.start_mission(request()).unwrap();
        assert_eq!(wait(&c, &id).status, "succeeded");
        drop(c);
        let primary = root.0.join("worktrees").join(&id);
        let ledger = root.0.join("runs").join(format!("{id}.ndjson"));
        let text = std::fs::read_to_string(&ledger).unwrap();
        let lines: Vec<_> = text.lines().collect();
        let snapshot = lines
            .iter()
            .position(|line| {
                let e: serde_json::Value = serde_json::from_str(line).unwrap();
                e["type"] == "workspace_snapshot"
            })
            .unwrap();
        let data: serde_json::Value = serde_json::from_str(lines[snapshot]).unwrap();
        let commits = data["data"]["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 3);
        let source_head = commits[2].as_str().unwrap();
        git(&primary, &["reset", "--hard", "HEAD~2"]);
        let turn = root.0.join("worktrees").join(format!(
            ".turn-{id}-{}-1",
            hex::encode(Sha256::digest(b"fmn_work"))
        ));
        git(
            &primary,
            &[
                "worktree",
                "add",
                "--detach",
                turn.to_str().unwrap(),
                source_head,
            ],
        );
        let cut = if event_kind == "workspace_snapshot" {
            snapshot
        } else {
            lines
                .iter()
                .position(|line| {
                    let e: serde_json::Value = serde_json::from_str(line).unwrap();
                    e["type"] == "workspace_commit"
                })
                .unwrap()
        };
        std::fs::write(&ledger, lines[..=cut].join("\n") + "\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let c = loop {
            match Coordinator::open(
                &root.0,
                settings.clone(),
                Box::new(Committer { dirty: true }),
            ) {
                Ok(c) => break c,
                Err(_) => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };
        assert_eq!(wait(&c, &id).status, "succeeded");
        let history = git(&primary, &["log", "--format=%s"]);
        assert_eq!(
            history
                .lines()
                .filter(|line| *line == "agent commit")
                .count(),
            1
        );
        assert_eq!(
            std::fs::read_to_string(primary.join("agent-file")).unwrap(),
            "committed by agent"
        );
        assert_eq!(
            std::fs::read_to_string(primary.join("dirty-file")).unwrap(),
            "uncommitted"
        );
        assert!(turns(&root, &id).is_empty());
    }
}

#[test]
fn recovery_unregisters_a_turn_whose_directory_was_already_removed() {
    use sha2::{Digest, Sha256};
    let root = Scratch::new();
    let settings = setup(&root);
    let c = Coordinator::open(
        &root.0,
        settings.clone(),
        Box::new(Committer { dirty: false }),
    )
    .unwrap();
    let id = c.start_mission(request()).unwrap();
    assert_eq!(wait(&c, &id).status, "succeeded");
    drop(c);
    let primary = root.0.join("worktrees").join(&id);
    let turn = root.0.join("worktrees").join(format!(
        ".turn-{id}-{}-1",
        hex::encode(Sha256::digest(b"fmn_work"))
    ));
    git(
        &primary,
        &[
            "worktree",
            "add",
            "--detach",
            turn.to_str().unwrap(),
            "HEAD",
        ],
    );
    std::fs::remove_dir_all(&turn).unwrap();
    let ledger = root.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&ledger).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let settled = lines
        .iter()
        .position(|line| {
            let e: serde_json::Value = serde_json::from_str(line).unwrap();
            e["type"] == "workspace_settled"
        })
        .unwrap();
    std::fs::write(&ledger, lines[..=settled].join("\n") + "\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(
            &root.0,
            settings.clone(),
            Box::new(Committer { dirty: false }),
        ) {
            Ok(c) => break c,
            Err(_) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    assert_eq!(wait(&c, &id).status, "succeeded");
    assert!(
        !git(&primary, &["worktree", "list", "--porcelain"])
            .contains(&turn.to_string_lossy().to_string())
    );
}
