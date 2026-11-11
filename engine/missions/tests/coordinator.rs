use parvi_missions::{
    Coordinator, ExecutionSink, Executor, FormationExecution, FormationResult, LabExecutor,
    RunSettings, RunStart,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        let mut b = [0; 8];
        getrandom::fill(&mut b).unwrap();
        let p = std::env::temp_dir().join(format!("parvi-99-coord-{}", u64::from_ne_bytes(b)));
        std::fs::create_dir_all(p.join("missions")).unwrap();
        std::fs::write(
            p.join("missions/hello.mission.toml"),
            include_str!("parity/runs/missions/solo-lab.mission.toml"),
        )
        .unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn request() -> RunStart {
    RunStart {
        mission: "hello".into(),
        input_card_id: "inp_hello".into(),
        expected_rev: 1,
        inputs: BTreeMap::from([("brief".into(), "hello".into())]),
        ..Default::default()
    }
}
fn wait_final(c: &Coordinator, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if c.run(id).unwrap().r#final {
            return;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn shutdown_block_with_completed_evidence_never_schedules_on_open() {
    struct CompletedOnly;
    impl Executor for CompletedOnly {
        fn execute_formation(
            &self,
            request: FormationExecution,
            sink: ExecutionSink,
        ) -> Result<FormationResult, String> {
            LabExecutor.execute_formation(request, sink)
        }
        fn recover_completed(
            &self,
            dispatch: parvi_missions::OpenDispatch,
        ) -> Option<FormationResult> {
            LabExecutor.recover_completed(dispatch)
        }
    }
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c.start_mission(request()).unwrap();
    wait_final(&c, &id);
    drop(c);
    let path = dir.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let cut = lines
        .iter()
        .position(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "slot_dispatch"
        })
        .unwrap();
    let dispatch: serde_json::Value = serde_json::from_str(lines[cut]).unwrap();
    std::fs::write(&path, lines[..=cut].join("\n") + "\n").unwrap();
    parvi_missions::Ledger::new(&path, &id).append(parvi_missions::RunEvent { kind:"run_blocked".into(), timestamp:"2026-01-01T00:00:00Z".into(), data:serde_json::json!({"code":"coordinator_shutdown","resumeAllowed":true,"openDispatches":[dispatch["data"]]}).as_object().unwrap().clone(), ..Default::default() }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(&dir.0, RunSettings::default(), Box::new(CompletedOnly)) {
            Ok(c) => break c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("open: {e}"),
        }
    };
    std::thread::sleep(Duration::from_millis(100));
    let p = c.run(&id).unwrap();
    assert_eq!(p.status, "blocked");
    assert!(
        !p.events
            .iter()
            .any(|e| e.kind == "run_resumed" || e.kind == "node_output" && e.node_id == "fmn_work")
    );
    std::fs::remove_file(
        dir.0
            .join("artifacts")
            .join(&id)
            .join("fmn_work/1/lab-completed.json"),
    )
    .unwrap();
    c.resume(&id, Default::default()).unwrap();
    wait_final(&c, &id);
}

#[test]
fn completed_dispatch_without_evidence_requires_reattach_or_redispatch() {
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c.start_mission(request()).unwrap();
    wait_final(&c, &id);
    drop(c);
    let path = dir.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let cut = lines
        .iter()
        .position(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "slot_result"
        })
        .unwrap();
    std::fs::write(&path, lines[..=cut].join("\n") + "\n").unwrap();
    std::fs::remove_file(
        dir.0
            .join("artifacts")
            .join(&id)
            .join("fmn_work/1/lab-completed.json"),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)) {
            Ok(c) => break c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("open: {e}"),
        }
    };
    let events = parvi_missions::Ledger::new(&path, &id).read().unwrap();
    let restart_block = events.iter().find(|e| e.kind == "run_blocked").unwrap();
    assert_eq!(
        restart_block.data["openDispatches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    loop {
        let p = c.run(&id).unwrap();
        if p.block_code == "dispatch_reattach_failed" {
            assert_eq!(
                p.events
                    .iter()
                    .filter(|e| e.kind == "slot_dispatch")
                    .count(),
                1
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    loop {
        match c.resume(
            &id,
            parvi_missions::ResumeRequest {
                mode: parvi_missions::ResumeMode::Redispatch,
                ..Default::default()
            },
        ) {
            Ok(_) => break,
            Err(e) if e.code == "conflict" && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("redispatch: {e}"),
        }
    }
    wait_final(&c, &id);
    let p = c.run(&id).unwrap();
    assert_eq!(p.status, "succeeded");
    assert!(
        p.events
            .iter()
            .any(|e| e.kind == "slot_result" && e.status == "abandoned")
    );
}

#[test]
fn failed_publication_at_startup_blocks_only_the_affected_run() {
    fn git(dir: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let dir = Scratch::new();
    let checkout = dir.0.join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    git(&checkout, &["init", "-b", "main"]);
    git(&checkout, &["config", "user.email", "test@example.invalid"]);
    git(&checkout, &["config", "user.name", "Test"]);
    std::fs::write(checkout.join("base"), "base").unwrap();
    git(&checkout, &["add", "base"]);
    git(&checkout, &["commit", "-m", "base"]);
    let settings = RunSettings {
        repositories: BTreeMap::from([("owner/repo".into(), checkout)]),
        ..Default::default()
    };
    let c = Coordinator::open(&dir.0, settings.clone(), Box::new(LabExecutor)).unwrap();
    let other = c.start_mission(request()).unwrap();
    wait_final(&c, &other);
    let mut start = request();
    start.repository = "owner/repo".into();
    let id = c.start_mission(start).unwrap();
    wait_final(&c, &id);
    drop(c);
    let path = dir.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let cut = lines
        .iter()
        .position(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "slot_dispatch"
        })
        .unwrap();
    std::fs::write(&path, lines[..=cut].join("\n") + "\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(&dir.0, settings.clone(), Box::new(LabExecutor)) {
            Ok(c) => break c,
            Err(e)
                if e.to_string().contains("another coordinator") && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("open: {e}"),
        }
    };
    assert!(c.run(&other).unwrap().r#final);
    let p = c.run(&id).unwrap();
    assert_eq!(p.status, "blocked");
    assert_eq!(p.block_code, "workspace_publication_failed");
    assert!(p.resume_allowed);
    let events = parvi_missions::Ledger::new(&path, &id).read().unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.kind == "node_output_pending")
    );
}

#[cfg(unix)]
#[test]
fn shutdown_deadline_covers_a_stalled_git_publication() {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = std::env::var_os("PARVI_101_STALLED_GIT") {
        let dir = PathBuf::from(dir);
        let marker = dir.join("started");
        let release = dir.join("release");
        let checkout = dir.join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(&checkout)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "test@example.invalid"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(checkout.join("base"), "base").unwrap();
        git(&["add", "base"]);
        git(&["commit", "-m", "base"]);
        let root = Scratch::new();
        let settings = RunSettings {
            repositories: BTreeMap::from([("owner/repo".into(), checkout)]),
            ..Default::default()
        };
        let c = Coordinator::open(&root.0, settings.clone(), Box::new(LabExecutor)).unwrap();
        let mut start = request();
        start.repository = "owner/repo".into();
        let id = c.start_mission(start).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let begin = Instant::now();
        assert_eq!(
            c.shutdown(begin + Duration::from_millis(100))
                .unwrap_err()
                .code,
            "timed_out"
        );
        assert!(begin.elapsed() < Duration::from_secs(2));
        drop(c);
        assert!(Coordinator::open(&root.0, settings.clone(), Box::new(LabExecutor)).is_err());
        std::fs::write(release, "release").unwrap();
        loop {
            match Coordinator::open(&root.0, settings.clone(), Box::new(LabExecutor)) {
                Ok(c) => {
                    assert_eq!(c.run(&id).unwrap().status, "blocked");
                    break;
                }
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                Err(e) => panic!("open: {e}"),
            }
        }
        return;
    }
    let mut random = [0; 8];
    getrandom::fill(&mut random).unwrap();
    let dir = std::env::temp_dir().join(format!(
        "parvi-101-stalled-git-{}",
        u64::from_ne_bytes(random)
    ));
    std::fs::create_dir(&dir).unwrap();
    let script = dir.join("git");
    std::fs::write(&script, "#!/bin/sh\nif [ \"$1\" = rev-list ]; then touch \"$PARVI_101_STALLED_GIT/started\"; while [ ! -e \"$PARVI_101_STALLED_GIT/release\" ]; do sleep 0.02; done; fi\nexec /usr/bin/git \"$@\"\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(dir.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "shutdown_deadline_covers_a_stalled_git_publication",
            "--nocapture",
        ])
        .env("PARVI_101_STALLED_GIT", &dir)
        .env("PATH", path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let timed_out = child.try_wait().unwrap().is_none();
    if timed_out {
        std::fs::write(dir.join("release"), "release").unwrap();
        let _ = child.kill();
    }
    let output = child.wait_with_output().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        !timed_out && output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn interrupted_dispatch_reattaches_completed_lab_evidence() {
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c.start_mission(request()).unwrap();
    wait_final(&c, &id);
    drop(c);
    let path = dir.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let cut = text
        .lines()
        .position(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "slot_dispatch"
        })
        .unwrap();
    std::fs::write(
        &path,
        text.lines().take(cut + 1).collect::<Vec<_>>().join("\n") + "\n",
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)) {
            Ok(c) => break c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("open: {e}"),
        }
    };
    assert!(resumed_after_restart(&dir.0, &id));
    wait_final(&c, &id);
    let p = c.run(&id).unwrap();
    assert_eq!(p.status, "succeeded");
    assert_eq!(
        p.events
            .iter()
            .filter(|e| e.kind == "slot_dispatch")
            .count(),
        1
    );
}

#[test]
fn missing_evidence_blocks_until_explicit_redispatch() {
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c.start_mission(request()).unwrap();
    wait_final(&c, &id);
    drop(c);
    let path = dir.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let dispatch = text
        .lines()
        .position(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "slot_dispatch"
        })
        .unwrap();
    std::fs::write(
        &path,
        text.lines()
            .take(dispatch + 1)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let evidence = dir
        .0
        .join("artifacts")
        .join(&id)
        .join("fmn_work/1/lab-completed.json");
    std::fs::remove_file(evidence).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)) {
            Ok(c) => break c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("open: {e}"),
        }
    };
    loop {
        let p = c.run(&id).unwrap();
        if p.block_code == "dispatch_reattach_failed" {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    loop {
        match c.resume(
            &id,
            parvi_missions::ResumeRequest {
                mode: parvi_missions::ResumeMode::Redispatch,
                ..Default::default()
            },
        ) {
            Ok(_) => break,
            Err(e) if e.code == "conflict" && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("resume: {e}"),
        }
    }
    wait_final(&c, &id);
    let p = c.run(&id).unwrap();
    assert_eq!(p.status, "succeeded");
    assert!(
        p.events
            .iter()
            .any(|e| e.kind == "slot_result" && e.status == "abandoned")
    );
    assert_eq!(
        p.events
            .iter()
            .filter(|e| e.kind == "slot_dispatch")
            .count(),
        2
    );
}

#[test]
fn shutdown_fences_work_and_leaves_active_run_resumable() {
    let dir = Scratch::new();
    let path = dir.0.join("missions/hello.mission.toml");
    let mission = std::fs::read_to_string(&path).unwrap().replace(
        "Return a short result",
        "archon-lab-work:10s Return a short result",
    );
    std::fs::write(path, mission).unwrap();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c.start_mission(request()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !c
        .run(&id)
        .unwrap()
        .events
        .iter()
        .any(|e| e.kind == "node_started" && e.node_id == "fmn_work")
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let start = Instant::now();
    c.shutdown(start + Duration::from_secs(8)).unwrap();
    assert!(start.elapsed() < Duration::from_secs(8));
    let p = c.run(&id).unwrap();
    assert_eq!(p.status, "blocked");
    assert!(p.resume_allowed);
    assert!(c.start_mission(request()).is_err());
}

#[test]
fn shutdown_timeout_keeps_the_writer_lock_until_worker_exits() {
    struct Stubborn(Arc<AtomicUsize>);
    impl Executor for Stubborn {
        fn execute_formation(
            &self,
            _: FormationExecution,
            _: ExecutionSink,
        ) -> Result<FormationResult, String> {
            self.0.store(1, Ordering::Release);
            std::thread::sleep(Duration::from_millis(400));
            Err("stopped".into())
        }
    }
    let dir = Scratch::new();
    let started = Arc::new(AtomicUsize::new(0));
    let c = Coordinator::open(
        &dir.0,
        RunSettings::default(),
        Box::new(Stubborn(started.clone())),
    )
    .unwrap();
    let id = c.start_mission(request()).unwrap();
    let timeout = Instant::now() + Duration::from_secs(5);
    while started.load(Ordering::Acquire) == 0 {
        assert!(Instant::now() < timeout);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        c.shutdown(Instant::now() + Duration::from_millis(50))
            .unwrap_err()
            .code,
        "timed_out"
    );
    drop(c);
    assert!(Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).is_err());
    let c = loop {
        match Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)) {
            Ok(c) => break c,
            Err(_) if Instant::now() < timeout => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("open: {e}"),
        }
    };
    assert_eq!(c.run(&id).unwrap().status, "blocked");
}

#[test]
fn detached_seat_is_ended_before_a_new_attempt() {
    struct Seats(Arc<std::sync::Mutex<Vec<String>>>);
    impl Executor for Seats {
        fn execute_formation(
            &self,
            req: FormationExecution,
            sink: ExecutionSink,
        ) -> Result<FormationResult, String> {
            LabExecutor.execute_formation(req, sink)
        }
        fn end_left_seat(&self, seat: parvi_missions::LeftSeat) -> Result<String, String> {
            assert_eq!(seat.identity["sessionId"], "$owned");
            self.0.lock().unwrap().push(seat.cause);
            Ok("ended".into())
        }
    }
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c.start_mission(request()).unwrap();
    wait_final(&c, &id);
    drop(c);
    let path = dir.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let cut = text
        .lines()
        .position(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "node_started"
                && line.contains("fmn_work")
        })
        .unwrap();
    std::fs::write(
        &path,
        text.lines().take(cut + 1).collect::<Vec<_>>().join("\n") + "\n",
    )
    .unwrap();
    let ledger = parvi_missions::Ledger::new(&path, &id);
    for (kind, data) in [
        (
            "seat_created",
            serde_json::json!({"sessionName":"scratch-seat","sessionId":"$owned"}),
        ),
        (
            "seat_cleanup",
            serde_json::json!({"sessionName":"scratch-seat","outcome":"left_shutdown"}),
        ),
    ] {
        ledger
            .append(parvi_missions::RunEvent {
                kind: kind.into(),
                timestamp: "2026-01-01T00:00:00Z".into(),
                node_id: "fmn_work".into(),
                slot_id: "slot_work".into(),
                data: data.as_object().unwrap().clone(),
                ..Default::default()
            })
            .unwrap();
    }
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(
            &dir.0,
            RunSettings::default(),
            Box::new(Seats(calls.clone())),
        ) {
            Ok(c) => break c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("open: {e}"),
        }
    };
    wait_final(&c, &id);
    assert_eq!(*calls.lock().unwrap(), ["new_attempt"]);
}

#[test]
fn aborting_a_blocked_run_ends_the_detached_seat() {
    struct Seats(Arc<std::sync::Mutex<Vec<String>>>);
    impl Executor for Seats {
        fn execute_formation(
            &self,
            req: FormationExecution,
            sink: ExecutionSink,
        ) -> Result<FormationResult, String> {
            LabExecutor.execute_formation(req, sink)
        }
        fn end_left_seat(&self, seat: parvi_missions::LeftSeat) -> Result<String, String> {
            self.0.lock().unwrap().push(seat.cause);
            Ok("ended".into())
        }
    }
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c.start_mission(request()).unwrap();
    wait_final(&c, &id);
    drop(c);
    let path = dir.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let cut = text
        .lines()
        .position(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "node_started"
                && line.contains("fmn_work")
        })
        .unwrap();
    std::fs::write(
        &path,
        text.lines().take(cut + 1).collect::<Vec<_>>().join("\n") + "\n",
    )
    .unwrap();
    let ledger = parvi_missions::Ledger::new(&path, &id);
    for (kind, data) in [
        (
            "seat_created",
            serde_json::json!({"sessionName":"scratch-seat","sessionId":"$owned"}),
        ),
        (
            "seat_cleanup",
            serde_json::json!({"sessionName":"scratch-seat","outcome":"left_shutdown"}),
        ),
    ] {
        ledger
            .append(parvi_missions::RunEvent {
                kind: kind.into(),
                timestamp: "2026-01-01T00:00:00Z".into(),
                node_id: "fmn_work".into(),
                slot_id: "slot_work".into(),
                data: data.as_object().unwrap().clone(),
                ..Default::default()
            })
            .unwrap();
    }
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(
            &dir.0,
            RunSettings::default(),
            Box::new(Seats(calls.clone())),
        ) {
            Ok(c) => break c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("open: {e}"),
        }
    };
    let p = c.abort(&id, "human:test", "stop").unwrap();
    assert_eq!(p.status, "canceled");
    assert_eq!(*calls.lock().unwrap(), ["run_final"]);
    assert!(
        p.events
            .iter()
            .any(|e| e.kind == "seat_cleanup" && e.outcome == "ended")
    );
}

#[test]
fn restart_at_each_ledger_cut_reaches_the_same_output() {
    use std::sync::Mutex;
    type Artifacts = Vec<(String, Vec<u8>)>;
    struct CutLab(Arc<Mutex<Vec<Artifacts>>>);
    impl Executor for CutLab {
        fn execute_formation(
            &self,
            req: FormationExecution,
            sink: ExecutionSink,
        ) -> Result<FormationResult, String> {
            let slot = &req.formation.slots[0];
            let dispatch = format!("dsp_{}_{}_{}", req.run_id, slot.id, req.attempt);
            std::fs::create_dir_all(&req.artifact_dir).map_err(|e| e.to_string())?;
            std::fs::write(req.artifact_dir.join("brief.md"), b"brief")
                .map_err(|e| e.to_string())?;
            let snapshot = || -> Result<Artifacts, String> {
                std::fs::read_dir(&req.artifact_dir)
                    .map_err(|e| e.to_string())?
                    .map(|entry| {
                        let entry = entry.map_err(|e| e.to_string())?;
                        Ok((
                            entry.file_name().to_string_lossy().into_owned(),
                            std::fs::read(entry.path()).map_err(|e| e.to_string())?,
                        ))
                    })
                    .collect()
            };
            sink.emit(parvi_missions::ExecutionEvent { kind:"slot_dispatch".into(), slot_id:slot.id.clone(), data:serde_json::json!({"dispatchId":dispatch,"nodeId":req.formation.id,"slotId":slot.id}).as_object().unwrap().clone() })?;
            self.0.lock().unwrap().push(snapshot()?);
            let result = FormationResult {
                status: "done".into(),
                text: "stable output".into(),
                outputs: req
                    .formation
                    .outputs
                    .iter()
                    .map(|port| (port.id.clone(), serde_json::json!({"text":"stable output"})))
                    .collect(),
            };
            std::fs::write(
                req.artifact_dir.join("lab-completed.json"),
                serde_json::to_vec(&result).unwrap(),
            )
            .map_err(|e| e.to_string())?;
            sink.emit(parvi_missions::ExecutionEvent { kind:"slot_result".into(), slot_id:slot.id.clone(), data:serde_json::json!({"dispatchId":dispatch,"status":"ok","sentinel":{"runId":req.run_id}}).as_object().unwrap().clone() })?;
            self.0.lock().unwrap().push(snapshot()?);
            Ok(result)
        }
        fn reattach(
            &self,
            dispatch: parvi_missions::OpenDispatch,
        ) -> Result<FormationResult, String> {
            LabExecutor.reattach(dispatch)
        }
        fn recover_completed(
            &self,
            dispatch: parvi_missions::OpenDispatch,
        ) -> Option<FormationResult> {
            LabExecutor.recover_completed(dispatch)
        }
    }
    let original = Scratch::new();
    let snapshots = Arc::new(Mutex::new(Vec::new()));
    let c = Coordinator::open(
        &original.0,
        RunSettings::default(),
        Box::new(CutLab(snapshots.clone())),
    )
    .unwrap();
    let id = c.start_mission(request()).unwrap();
    wait_final(&c, &id);
    drop(c);
    let source = original.0.join("runs");
    let ledger = std::fs::read_to_string(source.join(format!("{id}.ndjson"))).unwrap();
    let lines: Vec<_> = ledger.lines().collect();
    let index = |kind: &str| {
        lines
            .iter()
            .position(|line| {
                serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == kind
            })
            .unwrap()
    };
    let dispatch = index("slot_dispatch");
    let result = index("slot_result");
    let started = lines
        .iter()
        .position(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .is_ok_and(|event| event["type"] == "node_started" && event["nodeId"] == "fmn_work")
        })
        .unwrap();
    let captured = snapshots.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    for cut in 1..lines.len() {
        let scratch = Scratch::new();
        let runs = scratch.0.join("runs");
        std::fs::create_dir_all(&runs).unwrap();
        for suffix in ["snapshot.toml", "bindings.toml"] {
            std::fs::copy(
                source.join(format!("{id}.{suffix}")),
                runs.join(format!("{id}.{suffix}")),
            )
            .unwrap();
        }
        if cut > dispatch {
            let target = scratch.0.join("artifacts").join(&id).join("fmn_work/1");
            std::fs::create_dir_all(&target).unwrap();
            for (name, bytes) in &captured[usize::from(cut > result)] {
                std::fs::write(target.join(name), bytes).unwrap();
            }
        }
        std::fs::write(
            runs.join(format!("{id}.ndjson")),
            lines[..cut].join("\n") + "\n",
        )
        .unwrap();
        let c = Coordinator::open(
            &scratch.0,
            RunSettings::default(),
            Box::new(CutLab(Arc::new(Mutex::new(Vec::new())))),
        )
        .unwrap();
        assert!(resumed_after_restart(&scratch.0, &id), "cut {cut}");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let p = c.run(&id).unwrap();
            if p.r#final {
                assert_eq!(p.status, "succeeded", "cut {cut}");
                break;
            }
            if p.block_code == "dispatch_reattach_failed" {
                match c.resume(
                    &id,
                    parvi_missions::ResumeRequest {
                        mode: parvi_missions::ResumeMode::Redispatch,
                        ..Default::default()
                    },
                ) {
                    Ok(_) => {}
                    Err(e) if e.code == "conflict" => {}
                    Err(e) => panic!("resume: {e}"),
                }
            }
            assert!(Instant::now() < deadline, "cut {cut}");
            std::thread::sleep(Duration::from_millis(5));
        }
        let events = parvi_missions::Ledger::new(runs.join(format!("{id}.ndjson")), &id)
            .read()
            .unwrap();
        let outputs: Vec<_> = events
            .iter()
            .filter(|event| event.kind == "node_output")
            .map(|event| {
                (
                    event.node_id.as_str(),
                    event.data.get("status"),
                    event.data.get("text"),
                    event.data.get("outputs"),
                )
            })
            .collect();
        let baseline: Vec<serde_json::Value> = lines
            .iter()
            .filter_map(|line| {
                let event: serde_json::Value = serde_json::from_str(line).unwrap();
                (event["type"] == "node_output").then_some(event)
            })
            .collect();
        assert_eq!(outputs.len(), baseline.len(), "cut {cut}");
        for (actual, expected) in outputs.iter().zip(&baseline) {
            assert_eq!(actual.0, expected["nodeId"].as_str().unwrap());
            assert_eq!(actual.1, expected["data"].get("status"));
            assert_eq!(actual.2, expected["data"].get("text"));
            assert_eq!(actual.3, expected["data"].get("outputs"));
        }
        let dispatches: Vec<_> = events
            .iter()
            .filter(|event| event.kind == "slot_dispatch")
            .map(|event| {
                (
                    event.node_id.clone(),
                    event.slot_id.clone(),
                    event.attempt,
                    event.data["dispatchId"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let attempts = if cut == dispatch + 1 {
            vec![1, 2]
        } else if cut > started && cut <= dispatch {
            vec![2]
        } else {
            vec![1]
        };
        let expected_dispatches: Vec<_> = attempts
            .into_iter()
            .map(|attempt| {
                (
                    "fmn_work".to_owned(),
                    "slot_work".to_owned(),
                    attempt,
                    format!("dsp_{id}_slot_work_{attempt}"),
                )
            })
            .collect();
        assert_eq!(dispatches, expected_dispatches, "cut {cut}");
        assert_eq!(events.last().unwrap().kind, "run_succeeded");
    }
}

#[test]
fn restart_between_steps_matches_archon_lab() {
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c.start_mission(request()).unwrap();
    wait_final(&c, &id);
    drop(c);
    let path = dir.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let cut = text
        .lines()
        .position(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .is_ok_and(|e| e["type"] == "node_output" && e["nodeId"] == "inp_hello")
        })
        .unwrap();
    std::fs::write(
        &path,
        text.lines().take(cut + 1).collect::<Vec<_>>().join("\n") + "\n",
    )
    .unwrap();
    std::fs::remove_dir_all(dir.0.join("artifacts").join(&id)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)) {
            Ok(c) => break c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("open: {e}"),
        }
    };
    wait_final(&c, &id);
    let actual: Vec<serde_json::Value> = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            let mut normalized = serde_json::Map::new();
            for key in ["type", "nodeId", "gateId", "slotId", "attempt"] {
                if let Some(value) = event.get(key) {
                    normalized.insert(key.into(), value.clone());
                }
            }
            for key in ["status", "verdict", "outcome", "code"] {
                if let Some(value) = event["data"].get(key) {
                    normalized.insert(key.into(), value.clone());
                }
            }
            normalized.into()
        })
        .collect();
    let expected: Vec<serde_json::Value> =
        include_str!("parity/runs/archon-ledgers/restart-between-steps.ndjson")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(actual, expected);
}
#[test]
fn lab_run_completes_with_archon_sequence_and_lock_excludes_other_owners() {
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    assert!(Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).is_err());
    let id = c.start_mission(request()).unwrap();
    wait_final(&c, &id);
    let p = c.run(&id).unwrap();
    assert_eq!(p.status, "succeeded");
    assert_eq!(
        std::fs::read_to_string(dir.0.join("runs").join(format!("{id}.snapshot.toml"))).unwrap(),
        include_str!("parity/runs/missions/solo-lab.mission.toml")
    );
    assert_eq!(
        p.events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        [
            "run_started",
            "node_started",
            "node_output",
            "node_started",
            "slot_dispatch",
            "slot_result",
            "node_output",
            "run_succeeded"
        ]
    );
    let events = parvi_missions::Ledger::new(dir.0.join("runs").join(format!("{id}.ndjson")), &id)
        .read()
        .unwrap();
    let dispatch = events
        .iter()
        .find(|event| event.kind == "slot_dispatch")
        .unwrap();
    assert_eq!(dispatch.data["agentId"], "builder");
    let brief = std::path::Path::new(dispatch.data["briefPath"].as_str().unwrap());
    let brief_text = std::fs::read_to_string(brief).unwrap();
    assert!(brief_text.contains("input: hello"));
    assert!(brief_text.contains("role card:"));
    assert_eq!(dispatch.data["recordedBeforeSend"], true);
    assert_eq!(c.list(None, false).unwrap()[0].number, 1);
}
struct Waiter(Arc<AtomicUsize>);
impl Executor for Waiter {
    fn execute_formation(
        &self,
        r: FormationExecution,
        _events: ExecutionSink,
    ) -> Result<FormationResult, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        while !r.canceled.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.0.fetch_sub(1, Ordering::SeqCst);
        Err("canceled".into())
    }
}
#[test]
fn two_runs_overlap_and_abort_waits_for_executor() {
    let dir = Scratch::new();
    let active = Arc::new(AtomicUsize::new(0));
    let c = Coordinator::open(
        &dir.0,
        RunSettings::default(),
        Box::new(Waiter(active.clone())),
    )
    .unwrap();
    let a = c.start_mission(request()).unwrap();
    let b = c.start_mission(request()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while active.load(Ordering::SeqCst) < 2 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let p = c.abort(&a, "human:test", "stop").unwrap();
    assert_eq!(p.status, "canceled");
    assert_eq!(p.ended_by, "human:test");
    assert_eq!(active.load(Ordering::SeqCst), 1);
    c.abort(&b, "human:test", "stop").unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[test]
fn lab_work_duration_can_be_aborted_without_a_result_event() {
    let dir = Scratch::new();
    let path = dir.0.join("missions/hello.mission.toml");
    let raw = std::fs::read_to_string(&path).unwrap().replace(
        "Return a short result using the supplied output contract.",
        "archon-lab-work: 10s",
    );
    std::fs::write(path, raw).unwrap();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let id = c.start_mission(request()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if c.run(&id)
            .unwrap()
            .events
            .iter()
            .any(|e| e.kind == "node_started" && e.node_id == "fmn_work")
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let p = c.abort(&id, "human:test", "stop").unwrap();
    assert_eq!(p.status, "canceled");
    assert!(
        !p.events
            .iter()
            .any(|e| e.kind == "slot_result" || e.kind == "run_failed" || e.kind == "run_blocked")
    );
}

#[test]
fn rejected_admission_allocates_nothing_and_stale_revision_conflicts() {
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let mut req = request();
    req.inputs.clear();
    assert!(matches!(
        c.start_mission(req),
        Err(parvi_missions::AdmissionFailure::Findings(_))
    ));
    assert_eq!(std::fs::read_dir(dir.0.join("runs")).unwrap().count(), 0);
    assert!(!dir.0.join("workspaces").exists());
    let mut req = request();
    req.expected_rev = 2;
    assert!(matches!(
        c.start_mission(req),
        Err(parvi_missions::AdmissionFailure::Conflict)
    ));
    let mut req = request();
    req.context_paths.push("relative-context".into());
    assert!(matches!(
        c.start_mission(req),
        Err(parvi_missions::AdmissionFailure::Invalid(_))
    ));
    let mut req = request();
    req.work = "bad owner/repo#1".into();
    assert!(matches!(
        c.start_mission(req),
        Err(parvi_missions::AdmissionFailure::Invalid(_))
    ));
    // A bare Bead id names no store: the node resolves it before admission.
    let mut req = request();
    req.work = "ctx-12".into();
    assert!(matches!(
        c.start_mission(req),
        Err(parvi_missions::AdmissionFailure::Invalid(_))
    ));
    assert_eq!(std::fs::read_dir(dir.0.join("runs")).unwrap().count(), 0);
}
#[test]
fn a_step_placed_elsewhere_needs_a_repository_and_a_well_formed_place() {
    let dir = Scratch::new();
    let mission = dir.0.join("missions/hello.mission.toml");
    let source = std::fs::read_to_string(&mission).unwrap();
    let place = |device: &str| {
        let text = source.replacen(
            "id = \"fmn_work\"",
            &format!("id = \"fmn_work\"\ndevice = \"{device}\""),
            1,
        );
        std::fs::write(&mission, text).unwrap();
    };
    let settings = RunSettings {
        environment: "here/wsl".into(),
        ..Default::default()
    };
    let c = Coordinator::open(&dir.0, settings, Box::new(LabExecutor)).unwrap();
    let refused = |code: parvi_missions::Code| {
        matches!(c.start_mission(request()), Err(parvi_missions::AdmissionFailure::Findings(f))
            if f.iter().any(|x| x.code == code && x.node_id == "fmn_work"))
    };
    place("elsewhere");
    assert!(refused(parvi_missions::Code::UnknownDevice));
    place("elsewhere/windows");
    assert!(refused(parvi_missions::Code::PlacementWithoutRepository));
    // A device missing from a fleet the node knows whole is refused; names
    // compare exactly, as placement finds the step's node.
    let with_devices = |devices: &[&str]| {
        let mut req = request();
        req.known_devices = Some(devices.iter().map(|d| d.to_string()).collect());
        match c.start_mission(req) {
            Err(parvi_missions::AdmissionFailure::Findings(f)) => f
                .iter()
                .filter(|x| x.node_id == "fmn_work")
                .map(|x| x.code)
                .collect::<Vec<_>>(),
            other => panic!("admitted: {other:?}"),
        }
    };
    assert_eq!(
        with_devices(&["here", "Other"]),
        [parvi_missions::Code::UnknownDevice]
    );
    assert_eq!(
        with_devices(&["here", "Elsewhere"]),
        [parvi_missions::Code::UnknownDevice]
    );
    assert_eq!(
        with_devices(&["here", "elsewhere"]),
        [parvi_missions::Code::PlacementWithoutRepository]
    );
    assert_eq!(std::fs::read_dir(dir.0.join("runs")).unwrap().count(), 0);
    // The coordinator's own environment is not a placement.
    place("here/wsl");
    let id = c.start_mission(request()).unwrap();
    wait_final(&c, &id);
    assert_eq!(c.run(&id).unwrap().status, "succeeded");
}
#[test]
fn selected_formation_runs_without_replaying_the_input_card() {
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let mut req = request();
    req.input_card_id.clear();
    req.formation_id = "fmn_work".into();
    let id = c.start_formation(req).unwrap();
    wait_final(&c, &id);
    let p = c.run(&id).unwrap();
    assert_eq!(p.status, "succeeded");
    assert!(!p.events.iter().any(|e| e.node_id == "inp_hello"));
}

#[test]
fn subscribers_receive_each_durable_projection_in_order() {
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let mut rx = c.subscribe();
    let id = c.start_mission(request()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut counts = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(p) if p.run_id == id => {
                counts.push(p.event_count);
                if p.r#final {
                    break;
                }
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(e) => panic!("stream failed: {e}"),
        }
    }
    assert_eq!(counts, vec![1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn oversized_frozen_role_is_rejected_before_a_run_is_recorded() {
    let dir = Scratch::new();
    std::fs::create_dir_all(dir.0.join("roles")).unwrap();
    let role = format!(
        "schema = 1\n[card]\nid = \"builder\"\nkind = \"specialist\"\nstatus = \"active\"\nsummary = \"{}\"\n",
        "x".repeat(1 << 20)
    );
    std::fs::write(dir.0.join("roles/builder.toml"), role).unwrap();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let err = c.start_mission(request()).unwrap_err();
    assert!(
        err.to_string()
            .contains("persona snapshot exceeds byte limit")
    );
    assert_eq!(std::fs::read_dir(dir.0.join("runs")).unwrap().count(), 0);
    assert!(!dir.0.join("workspaces").exists());
}

struct VerifyLease {
    root: PathBuf,
    recorded: Arc<std::sync::atomic::AtomicBool>,
}
impl Executor for VerifyLease {
    fn execute_formation(
        &self,
        request: FormationExecution,
        events: ExecutionSink,
    ) -> Result<FormationResult, String> {
        let dispatch = parvi_missions::ExecutionEvent {
            kind: "slot_dispatch".into(),
            slot_id: request.formation.slots[0].id.clone(),
            data: serde_json::json!({"dispatchId":"dsp_test"})
                .as_object()
                .unwrap()
                .clone(),
        };
        events.emit(dispatch)?;
        let ledger = parvi_missions::Ledger::new(
            self.root
                .join("runs")
                .join(format!("{}.ndjson", request.run_id)),
            &request.run_id,
        );
        let recorded = ledger
            .read()
            .map_err(|e| e.to_string())?
            .last()
            .is_some_and(|event| event.kind == "slot_dispatch");
        self.recorded.store(recorded, Ordering::Release);
        events.emit(parvi_missions::ExecutionEvent { kind:"slot_result".into(),slot_id:request.formation.slots[0].id.clone(),data:serde_json::json!({"dispatchId":"dsp_test","sentinel":{"runId":request.run_id,"status":"ok"}}).as_object().unwrap().clone() })?;
        let outputs = request
            .formation
            .outputs
            .iter()
            .map(|port| (port.id.clone(), serde_json::json!({"text":"done"})))
            .collect();
        Ok(FormationResult {
            status: "done".into(),
            text: "done".into(),
            outputs,
        })
    }
}
#[test]
fn executor_cannot_send_work_until_dispatch_is_durable() {
    let dir = Scratch::new();
    let recorded = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let executor = VerifyLease {
        root: dir.0.clone(),
        recorded: recorded.clone(),
    };
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(executor)).unwrap();
    let id = c.start_mission(request()).unwrap();
    wait_final(&c, &id);
    assert!(recorded.load(Ordering::Acquire));
}

struct MissingOutput;
impl Executor for MissingOutput {
    fn execute_formation(
        &self,
        request: FormationExecution,
        events: ExecutionSink,
    ) -> Result<FormationResult, String> {
        let slot_id = request.formation.slots[0].id.clone();
        events.emit(parvi_missions::ExecutionEvent {
            kind: "slot_dispatch".into(),
            slot_id: slot_id.clone(),
            data: serde_json::json!({"dispatchId":"dsp_missing_output"})
                .as_object()
                .unwrap()
                .clone(),
        })?;
        events.emit(parvi_missions::ExecutionEvent{kind:"slot_result".into(),slot_id,data:serde_json::json!({"dispatchId":"dsp_missing_output","sentinel":{"runId":request.run_id,"status":"ok"}}).as_object().unwrap().clone()})?;
        Ok(FormationResult {
            status: "done".into(),
            text: "incomplete".into(),
            outputs: serde_json::Map::new(),
        })
    }
}
#[test]
fn a_formation_cannot_succeed_without_its_declared_outputs() {
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(MissingOutput)).unwrap();
    let id = c.start_mission(request()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let p = c.run(&id).unwrap();
        if p.status == "blocked" {
            assert!(!p.r#final);
            assert!(!p.events.iter().any(|e| e.kind == "run_succeeded"));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn a_run_that_blocks_is_told_once_with_its_mission_and_why() {
    let dir = Scratch::new();
    let told = Arc::new(std::sync::Mutex::new(Vec::new()));
    let tell = told.clone();
    let settings = RunSettings {
        on_block: Some(parvi_missions::OnBlock(Arc::new(move |run, reason| {
            tell.lock().unwrap().push((
                run.run_id.clone(),
                run.mission_slug.clone(),
                run.status.clone(),
                reason.to_owned(),
            ));
        }))),
        ..Default::default()
    };
    let c = Coordinator::open(&dir.0, settings, Box::new(MissingOutput)).unwrap();
    let id = c.start_mission(request()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while told.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    // The worker has stopped with the block; nothing more is told.
    c.shutdown(Instant::now() + Duration::from_secs(5)).unwrap();
    let told = told.lock().unwrap();
    assert_eq!(told.len(), 1, "{told:?}");
    let (run, mission, status, reason) = &told[0];
    assert_eq!(
        (run, mission.as_str(), status.as_str()),
        (&id, "hello", "blocked")
    );
    assert!(!reason.is_empty());
}

struct UnknownDispatch;
impl Executor for UnknownDispatch {
    fn execute_formation(
        &self,
        request: FormationExecution,
        events: ExecutionSink,
    ) -> Result<FormationResult, String> {
        events.emit(parvi_missions::ExecutionEvent{kind:"slot_result".into(),slot_id:request.formation.slots[0].id.clone(),data:serde_json::json!({"dispatchId":"missing","sentinel":{"runId":request.run_id,"status":"ok"}}).as_object().unwrap().clone()})?;
        unreachable!()
    }
}
#[test]
fn orphan_results_are_rejected_without_recording_a_slot_result() {
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(UnknownDispatch)).unwrap();
    let id = c.start_mission(request()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let p = c.run(&id).unwrap();
        if p.status == "blocked" {
            assert!(!p.events.iter().any(|e| e.kind == "slot_result"));
            assert!(
                p.events
                    .iter()
                    .any(|e| e.kind == "error" && e.code == "unknown_dispatch")
            );
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct WrongRunSentinel;
impl Executor for WrongRunSentinel {
    fn execute_formation(
        &self,
        request: FormationExecution,
        events: ExecutionSink,
    ) -> Result<FormationResult, String> {
        let slot_id = request.formation.slots[0].id.clone();
        events.emit(parvi_missions::ExecutionEvent {
            kind: "slot_dispatch".into(),
            slot_id: slot_id.clone(),
            data: serde_json::json!({"dispatchId":"dsp_wrong_run"})
                .as_object()
                .unwrap()
                .clone(),
        })?;
        events.emit(parvi_missions::ExecutionEvent{kind:"slot_result".into(),slot_id,data:serde_json::json!({"dispatchId":"dsp_wrong_run","sentinel":{"runId":"run_wrong","status":"ok"}}).as_object().unwrap().clone()})?;
        unreachable!()
    }
}
#[test]
fn completion_for_another_run_never_completes_a_dispatch() {
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(WrongRunSentinel)).unwrap();
    let id = c.start_mission(request()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let p = c.run(&id).unwrap();
        if p.status == "blocked" {
            assert!(
                p.events
                    .iter()
                    .any(|e| e.kind == "error" && e.code == "completion_sentinel_timeout")
            );
            assert!(!p.events.iter().any(|e| e.kind == "slot_result"));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn repository_runs_isolate_commit_and_land() {
    fn git(dir: &std::path::Path, args: &[&str]) -> String {
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
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }
    struct WritingLab;
    impl Executor for WritingLab {
        fn execute_formation(
            &self,
            req: FormationExecution,
            sink: ExecutionSink,
        ) -> Result<FormationResult, String> {
            std::fs::write(req.cwd.join(&req.formation.id), &req.run_id)
                .map_err(|e| e.to_string())?;
            LabExecutor.execute_formation(req, sink)
        }
    }
    let dir = Scratch::new();
    let mission_path = dir.0.join("missions/hello.mission.toml");
    let mission = std::fs::read_to_string(&mission_path).unwrap();
    let formation = mission
        .split("[[formation]]")
        .nth(1)
        .unwrap()
        .split("[[end]]")
        .next()
        .unwrap();
    let second = format!(
        "[[formation]]{}",
        formation
            .replace("fmn_work", "fmn_second")
            .replace("slot_work", "slot_second")
    );
    let mission = mission
        .replace("[[end]]", &format!("{second}[[end]]"))
        .replace("to = \"end_done:in\"", "to = \"fmn_second:port_in\"");
    let mission = format!(
        "{mission}\n[[connection]]\nid = \"edge_second\"\nfrom = \"fmn_second:port_out\"\nto = \"end_done:in\"\n"
    );
    std::fs::write(mission_path, mission).unwrap();
    let checkout = dir.0.join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    git(&checkout, &["init", "-b", "main"]);
    git(&checkout, &["config", "user.email", "test@example.invalid"]);
    git(&checkout, &["config", "user.name", "Test"]);
    std::fs::write(checkout.join("base"), "base").unwrap();
    git(&checkout, &["add", "base"]);
    git(&checkout, &["commit", "-m", "base"]);
    let original = git(&checkout, &["rev-parse", "HEAD"]);
    let settings = RunSettings {
        repositories: BTreeMap::from([("owner/repo".into(), checkout.clone())]),
        ..Default::default()
    };
    let c = Coordinator::open(&dir.0, settings, Box::new(WritingLab)).unwrap();
    let mut req = request();
    req.repository = "missing/repo".into();
    let err = c.start_mission(req.clone()).unwrap_err();
    assert!(
        matches!(err, parvi_missions::AdmissionFailure::Findings(ref f) if f.iter().any(|x| x.code == parvi_missions::Code::NoCheckout))
    );
    req.repository = "owner/repo".into();
    let a = c.start_mission(req.clone()).unwrap();
    let b = c.start_mission(req).unwrap();
    wait_final(&c, &a);
    wait_final(&c, &b);
    for id in [&a, &b] {
        let run = c.run(id).unwrap();
        assert_eq!(run.status, "succeeded");
        assert_eq!(run.cwd, dir.0.join("worktrees").join(id).to_string_lossy());
        let commits: Vec<_> = run
            .events
            .iter()
            .filter(|e| e.kind == "workspace_commit")
            .collect();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].node_id, "fmn_work");
        assert_eq!(commits[1].node_id, "fmn_second");
        assert!(commits.iter().all(|e| e.attempt == 1));
        assert!(
            git(
                std::path::Path::new(&run.cwd),
                &["log", "-1", "--format=%s"]
            )
            .contains("fmn_second")
        );
        assert!(c.land(id, false).is_err());
    }
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), original);
    assert_eq!(git(&checkout, &["branch", "--show-current"]), "main");
    assert!(!checkout.join("fmn_work").exists());
    git(
        &checkout,
        &["merge", "--no-ff", "-m", "land", &format!("parvi/run-{a}")],
    );
    c.land(&a, false).unwrap();
    c.land(&b, true).unwrap();
    for id in [&a, &b] {
        assert!(!dir.0.join("worktrees").join(id).exists());
        let branches = git(&checkout, &["branch", "--list", &format!("parvi/run-{id}")]);
        assert!(branches.is_empty());
    }
}

#[test]
fn recovery_publishes_an_output_recorded_before_its_commit() {
    use sha2::{Digest, Sha256};
    fn git(dir: &std::path::Path, args: &[&str]) {
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
    }
    struct Writing;
    impl Executor for Writing {
        fn execute_formation(
            &self,
            req: FormationExecution,
            sink: ExecutionSink,
        ) -> Result<FormationResult, String> {
            std::fs::write(req.cwd.join("turn-file"), "accepted turn")
                .map_err(|e| e.to_string())?;
            LabExecutor.execute_formation(req, sink)
        }
    }
    let dir = Scratch::new();
    let checkout = dir.0.join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    git(&checkout, &["init", "-b", "main"]);
    git(&checkout, &["config", "user.email", "test@example.invalid"]);
    git(&checkout, &["config", "user.name", "Test"]);
    std::fs::write(checkout.join("base"), "base").unwrap();
    git(&checkout, &["add", "base"]);
    git(&checkout, &["commit", "-m", "base"]);
    let settings = RunSettings {
        repositories: BTreeMap::from([("owner/repo".into(), checkout)]),
        ..Default::default()
    };
    let c = Coordinator::open(&dir.0, settings.clone(), Box::new(Writing)).unwrap();
    let mut start = request();
    start.repository = "owner/repo".into();
    let id = c.start_mission(start).unwrap();
    wait_final(&c, &id);
    drop(c);
    let path = dir.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let output = events
        .iter()
        .position(|e| e["type"] == "node_output_pending" && e["nodeId"] == "fmn_work")
        .unwrap();
    std::fs::write(
        &path,
        text.lines().take(output + 1).collect::<Vec<_>>().join("\n") + "\n",
    )
    .unwrap();
    let workspace = dir.0.join("worktrees").join(&id);
    git(&workspace, &["reset", "--hard", "HEAD^"]);
    let turn = dir
        .0
        .join("worktrees")
        .join(format!(".turn-{id}-{:x}-1", Sha256::digest(b"fmn_work")));
    git(
        &workspace,
        &[
            "worktree",
            "add",
            "--detach",
            turn.to_str().unwrap(),
            "HEAD",
        ],
    );
    std::fs::write(turn.join("turn-file"), "accepted turn").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let c = loop {
        match Coordinator::open(&dir.0, settings.clone(), Box::new(Writing)) {
            Ok(c) => break c,
            Err(_) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    assert!(resumed_after_restart(&dir.0, &id));
    wait_final(&c, &id);
    let p = c.run(&id).unwrap();
    assert_eq!(p.status, "succeeded");
    assert_eq!(
        std::fs::read_to_string(workspace.join("turn-file")).unwrap(),
        "accepted turn"
    );
    assert_eq!(
        p.events
            .iter()
            .filter(|e| e.kind == "node_output" && e.node_id == "fmn_work")
            .count(),
        1
    );
    assert!(
        p.events
            .iter()
            .any(|e| e.kind == "workspace_commit" && e.node_id == "fmn_work")
    );
}

#[test]
fn a_start_repeated_with_its_request_id_answers_the_run_it_made() {
    // A client that lost the answer to a start sends it again with the
    // same key: it gets the run already made, never a second run; a new
    // key starts a new run.
    let dir = Scratch::new();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let keyed = |key: &str| RunStart {
        request_id: key.into(),
        ..request()
    };
    let first = c.start_mission(keyed("req-1")).unwrap();
    wait_final(&c, &first);
    assert_eq!(c.start_mission(keyed("req-1")).unwrap(), first);
    assert_eq!(c.list(None, false).unwrap().len(), 1);
    assert_eq!(c.run(&first).unwrap().request_id, "req-1");
    let second = c.start_mission(keyed("req-2")).unwrap();
    assert_ne!(second, first);
    wait_final(&c, &second);
    assert_eq!(c.list(None, false).unwrap().len(), 2);
}

/// The reopened coordinator resumed the run itself, as #123 decided.
fn resumed_after_restart(root: &std::path::Path, id: &str) -> bool {
    parvi_missions::Ledger::new(root.join("runs").join(format!("{id}.ndjson")), id)
        .read()
        .unwrap()
        .iter()
        .any(|e| e.kind == "run_resumed" && e.actor == "parvi:restart")
}
