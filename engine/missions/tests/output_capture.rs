use parvi_mission_engine::{
    Coordinator, ExecutionEvent, ExecutionSink, Executor, FormationExecution, FormationResult,
    RunSettings,
};
use parvi_mission_model::{ResumeRequest, RunStart};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new(kind: &str) -> Self {
        let mut b = [0; 8];
        getrandom::fill(&mut b).unwrap();
        let root =
            std::env::temp_dir().join(format!("parvi-102-capture-{}", u64::from_ne_bytes(b)));
        std::fs::create_dir_all(root.join("missions")).unwrap();
        let output_name = if kind == "folder" { "site" } else { "report" };
        let mission = include_str!("parity/runs/missions/solo-lab.mission.toml")
            .replace("goal = \"Return the supplied input\"", &format!("goal = \"Return the supplied input\"\noutputs = [{{ name = \"{output_name}\", kind = \"{kind}\", from = \"fmn_work:port_work_out\", required = true }}]"))
            .replace("port_out", "port_work_out")
            .replace("label = \"Result\"", &format!("label = \"Result\"\nkind = \"{kind}\""));
        std::fs::write(root.join("missions/hello.mission.toml"), mission).unwrap();
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fn thaw(p: &std::path::Path) {
            if let Ok(m) = std::fs::symlink_metadata(p)
                && m.is_dir()
            {
                let mut perms = m.permissions();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    perms.set_mode(perms.mode() | 0o700);
                }
                #[cfg(windows)]
                {
                    // Clears the Windows readonly attribute; the lint's
                    // world-writable risk is Unix's, handled above.
                    #[allow(clippy::permissions_set_readonly_false)]
                    perms.set_readonly(false);
                }
                std::fs::set_permissions(p, perms).unwrap();
                for e in std::fs::read_dir(p).unwrap() {
                    thaw(&e.unwrap().path());
                }
            }
        }
        thaw(&self.0);
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Output {
    source: PathBuf,
    count: Arc<AtomicUsize>,
}
impl Executor for Output {
    fn execute_formation(
        &self,
        req: FormationExecution,
        sink: ExecutionSink,
    ) -> Result<FormationResult, parvi_mission_engine::Error> {
        (|| -> Result<FormationResult, String> {
            self.count.fetch_add(1, Ordering::SeqCst);
            let slot = req.formation.slots[0].id.clone();
            for (kind, data) in [
                ("slot_dispatch", json!({"dispatchId":"test"})),
                (
                    "slot_result",
                    json!({"dispatchId":"test","sentinel":{"runId":req.run_id,"status":"ok"}}),
                ),
            ] {
                sink.emit(ExecutionEvent {
                    kind: kind.into(),
                    slot_id: slot.clone(),
                    data: data.as_object().unwrap().clone(),
                })
                .map_err(|error| error.message)?;
            }
            Ok(FormationResult {
                status: "done".into(),
                text: "done".into(),
                outputs: json!({"port_work_out":{"text":"done","ref":self.source}})
                    .as_object()
                    .unwrap()
                    .clone(),
            })
        })()
        .map_err(|message: String| parvi_mission_engine::Error::new("executor_error", message))
    }
    fn reattach(
        &self,
        _: parvi_mission_engine::OpenDispatch,
    ) -> Result<FormationResult, parvi_mission_engine::Error> {
        Ok(FormationResult {
            status: "done".into(),
            text: "done".into(),
            outputs: json!({"port_work_out":{"text":"done","ref":self.source}})
                .as_object()
                .unwrap()
                .clone(),
        })
        .map_err(|message: String| parvi_mission_engine::Error::new("executor_error", message))
    }
}

#[test]
fn failed_capture_after_reattach_remains_redispatchable() {
    let root = Scratch::new("folder");
    let source = root.0.join("site");
    std::fs::create_dir(&source).unwrap();
    let file = source.join("large");
    let f = std::fs::File::create(&file).unwrap();
    f.set_len(32 * 1024 * 1024 + 1).unwrap();
    drop(f);
    let (c, id, count) = start(&root, source.clone());
    wait(&c, &id, "blocked");
    drop(c);
    let path = root.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let cut = lines
        .iter()
        .position(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "slot_dispatch"
        })
        .unwrap();
    std::fs::write(&path, lines[..=cut].join("\n") + "\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let c = loop {
        match Coordinator::open(
            &root.0,
            RunSettings::default(),
            Box::new(Output {
                source: source.clone(),
                count: count.clone(),
            }),
        ) {
            Ok(c) => break c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("open: {e}"),
        }
    };
    loop {
        let p = c.run(&id).unwrap();
        if p.block_code == "output_not_kept" {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let events = parvi_mission_engine::Ledger::new(&path, &id)
        .read()
        .unwrap();
    assert_eq!(
        events.last().unwrap().data["pendingOutput"]["nodeId"],
        "fmn_work"
    );
    std::fs::write(file, "narrowed").unwrap();
    loop {
        match c.resume(
            &id,
            ResumeRequest {
                mode: parvi_mission_model::ResumeMode::Redispatch,
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
    let p = wait(&c, &id, "succeeded");
    assert_eq!(p.outputs[0].attempt, 2);
}
fn start(root: &Scratch, source: PathBuf) -> (Coordinator, String, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let c = Coordinator::open(
        &root.0,
        RunSettings::default(),
        Box::new(Output {
            source,
            count: count.clone(),
        }),
    )
    .unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: [("brief".into(), "hello".into())].into_iter().collect(),
            ..Default::default()
        })
        .unwrap();
    (c, id, count)
}
fn wait(c: &Coordinator, id: &str, status: &str) -> parvi_mission_model::RunProjection {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        let p = c.run(id).unwrap();
        if p.status == status {
            return p;
        }
        assert!(Instant::now() < until, "status: {}", p.status);
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn file_output_is_kept_independently_and_projected() {
    let root = Scratch::new("file");
    let source = root.0.join("site");
    std::fs::write(&source, "draft 1").unwrap();
    let (c, id, _) = start(&root, source.clone());
    let run = wait(&c, &id, "succeeded");
    assert_eq!(run.outputs.len(), 1);
    let copy = PathBuf::from(&run.outputs[0].value);
    assert!(
        copy.starts_with(
            root.0
                .join("artifacts")
                .join(&id)
                .join("fmn_work/1/outputs/port_work_out")
        )
    );
    std::fs::write(source, "second").unwrap();
    let baseline: serde_json::Value =
        serde_json::from_str(include_str!("parity/runs/archon-ledgers/outputs.json")).unwrap();
    assert_eq!(
        std::fs::read_to_string(copy).unwrap(),
        baseline["file-output-kept"]["firstCopy"].as_str().unwrap()
    );
    assert_eq!(c.list(None, false).unwrap()[0].outputs, run.outputs);
}
#[test]
fn over_budget_blocks_then_resume_keeps_without_dispatching_again() {
    let root = Scratch::new("folder");
    let source = root.0.join("site");
    std::fs::create_dir(&source).unwrap();
    let file = source.join("large");
    let f = std::fs::File::create(&file).unwrap();
    f.set_len(32 * 1024 * 1024 + 1).unwrap();
    drop(f);
    let (c, id, count) = start(&root, source);
    let run = wait(&c, &id, "blocked");
    let baseline: serde_json::Value =
        serde_json::from_str(include_str!("parity/runs/archon-ledgers/outputs.json")).unwrap();
    let expected = &baseline["folder-over-budget"];
    assert_eq!(run.block_code, expected["blockedCode"].as_str().unwrap());
    std::fs::write(file, "narrowed").unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match c.resume(&id, ResumeRequest::default()) {
            Ok(_) => break,
            Err(e) if e.code == "conflict" && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => panic!("resume: {e}"),
        }
    }
    let run = wait(&c, &id, "succeeded");
    assert_eq!(
        count.load(Ordering::SeqCst) as u64,
        expected["dispatches"].as_u64().unwrap()
    );
    assert_eq!(run.status, expected["status"].as_str().unwrap());
    let mut output = serde_json::to_value(&run.outputs[0]).unwrap();
    let artifact_root = root.0.join("artifacts").join(&id);
    let relative = PathBuf::from(output["value"].as_str().unwrap())
        .strip_prefix(artifact_root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    output["value"] = json!(format!("<artifacts>/{relative}"));
    assert_eq!(output, expected["outputs"][0]);
    assert_eq!(
        std::fs::read_to_string(PathBuf::from(&run.outputs[0].value).join("large")).unwrap(),
        "narrowed"
    );
}

#[test]
fn redispatch_runs_a_new_attempt_instead_of_keeping_the_pending_one() {
    let root = Scratch::new("folder");
    let source = root.0.join("site");
    std::fs::create_dir(&source).unwrap();
    let file = source.join("large");
    let f = std::fs::File::create(&file).unwrap();
    f.set_len(32 * 1024 * 1024 + 1).unwrap();
    drop(f);
    let (c, id, count) = start(&root, source);
    let run = wait(&c, &id, "blocked");
    assert_eq!(run.block_code, "output_not_kept");
    let ledger =
        parvi_mission_engine::Ledger::new(root.0.join("runs").join(format!("{id}.ndjson")), &id)
            .read()
            .unwrap();
    let block = ledger
        .iter()
        .find(|event| event.kind == "run_blocked")
        .unwrap();
    assert_eq!(block.data["pendingOutput"]["attempt"], 1);
    std::fs::write(file, "narrowed").unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match c.resume(
            &id,
            ResumeRequest {
                mode: parvi_mission_model::ResumeMode::Redispatch,
                ..Default::default()
            },
        ) {
            Ok(_) => break,
            Err(e) if e.code == "conflict" && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => panic!("resume: {e}"),
        }
    }
    let run = wait(&c, &id, "succeeded");
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(run.outputs[0].attempt, 2);
}

#[test]
fn restart_blocks_an_interrupted_copy_with_its_pending_output() {
    let root = Scratch::new("file");
    std::fs::create_dir_all(root.0.join("runs")).unwrap();
    let ledger = parvi_mission_engine::Ledger::new(
        root.0.join("runs/run_interrupted.ndjson"),
        "run_interrupted",
    );
    ledger
        .create(parvi_mission_engine::RunEvent {
            kind: "run_started".into(),
            timestamp: "2026-11-06T00:00:00Z".into(),
            actor: "agent:test".into(),
            ..Default::default()
        })
        .unwrap();
    ledger
        .append(parvi_mission_engine::RunEvent {
            kind: "output_capture_pending".into(),
            node_id: "fmn_work".into(),
            attempt: 1,
            timestamp: "2026-11-06T00:00:01Z".into(),
            data: json!({"status":"done","outputs":{"port_work_out":{"ref":"/source"}}})
                .as_object()
                .unwrap()
                .clone(),
            ..Default::default()
        })
        .unwrap();
    let c = Coordinator::open(
        &root.0,
        RunSettings::default(),
        Box::new(parvi_mission_engine::LabExecutor),
    )
    .unwrap();
    let run = c.run("run_interrupted").unwrap();
    assert_eq!(run.status, "blocked");
    assert_eq!(run.block_code, "coordinator_shutdown");
    let events = ledger.read().unwrap();
    let block = events.last().unwrap();
    assert_eq!(block.data["pendingOutput"]["nodeId"], "fmn_work");
    assert_eq!(block.data["pendingOutput"]["attempt"], 1);
}

#[test]
fn file_refs_must_be_absolute_existing_regular_files() {
    for (case, code) in [
        ("relative", "invalid_output_ref"),
        ("missing", "unavailable_output_ref"),
        ("directory", "invalid_output_ref"),
    ] {
        let root = Scratch::new("file");
        let source = match case {
            "relative" => PathBuf::from("relative"),
            "missing" => root.0.join("missing"),
            _ => root.0.clone(),
        };
        let (c, id, _) = start(&root, source);
        let run = wait(&c, &id, "blocked");
        assert_eq!(run.block_code, code, "{case}");
        assert!(
            !run.events
                .iter()
                .any(|event| event.kind == "node_output" && event.node_id == "fmn_work")
        );
    }
}

struct InvalidOnce(Output);
impl Executor for InvalidOnce {
    fn execute_formation(
        &self,
        req: FormationExecution,
        sink: ExecutionSink,
    ) -> Result<FormationResult, parvi_mission_engine::Error> {
        (|| -> Result<FormationResult, String> {
            let mut result = self
                .0
                .execute_formation(req, sink)
                .map_err(|error| error.message)?;
            if self.0.count.load(Ordering::SeqCst) == 1 {
                result.outputs["port_work_out"]["ref"] = json!("relative");
            }
            Ok(result)
        })()
        .map_err(|message: String| parvi_mission_engine::Error::new("executor_error", message))
    }
}
#[test]
fn invalid_reference_can_resume_by_running_the_step_again() {
    let root = Scratch::new("file");
    let source = root.0.join("result");
    std::fs::write(&source, "fixed").unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let c = Coordinator::open(
        &root.0,
        RunSettings::default(),
        Box::new(InvalidOnce(Output {
            source,
            count: count.clone(),
        })),
    )
    .unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: [("brief".into(), "hello".into())].into_iter().collect(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(wait(&c, &id, "blocked").block_code, "invalid_output_ref");
    let ledger =
        parvi_mission_engine::Ledger::new(root.0.join("runs").join(format!("{id}.ndjson")), &id);
    assert!(
        !ledger
            .read()
            .unwrap()
            .iter()
            .any(|e| e.kind == "output_capture_pending")
    );
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match c.resume(&id, ResumeRequest::default()) {
            Ok(_) => break,
            Err(e) if e.code == "conflict" && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => panic!("resume: {e}"),
        }
    }
    let run = wait(&c, &id, "succeeded");
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(run.outputs[0].attempt, 2);
}

#[test]
fn interrupted_replacement_restores_the_original_source_before_recapture() {
    let root = Scratch::new("file");
    let source = root.0.join("seed");
    std::fs::write(&source, "seed").unwrap();
    let (c, id, _) = start(&root, source);
    wait(&c, &id, "succeeded");
    let original =
        parvi_mission_engine::Ledger::new(root.0.join("runs").join(format!("{id}.ndjson")), &id)
            .read()
            .unwrap();
    drop(c);
    let new_id = "run_replacement";
    for suffix in ["snapshot.toml", "bindings.toml"] {
        std::fs::copy(
            root.0.join("runs").join(format!("{id}.{suffix}")),
            root.0.join("runs").join(format!("{new_id}.{suffix}")),
        )
        .unwrap();
    }
    let attempt = root.0.join("artifacts").join(new_id).join("fmn_work/1");
    let outputs = attempt.join("outputs");
    let saved = outputs.join("old/source");
    std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
    std::fs::write(&saved, "original").unwrap();
    let ledger = parvi_mission_engine::Ledger::new(
        root.0.join("runs").join(format!("{new_id}.ndjson")),
        new_id,
    );
    ledger.create(original[0].clone()).unwrap();
    for e in original
        .iter()
        .skip(1)
        .take_while(|e| !(e.kind == "node_started" && e.node_id == "fmn_work"))
    {
        ledger.append(e.clone()).unwrap();
    }
    let started = original
        .iter()
        .find(|e| e.kind == "node_started" && e.node_id == "fmn_work")
        .unwrap();
    ledger.append(started.clone()).unwrap();
    let event = |kind: &str, data: serde_json::Value| parvi_mission_engine::RunEvent {
        kind: kind.into(),
        node_id: "fmn_work".into(),
        attempt: 1,
        timestamp: "2026-11-06T00:00:01Z".into(),
        data: data.as_object().unwrap().clone(),
        ..Default::default()
    };
    ledger.append(event("output_capture_pending", json!({"status":"done","text":"done","outputs":{"port_work_out":{"text":"done","ref":saved}}}))).unwrap();
    let stage = attempt.join(".capture-interrupted");
    std::fs::create_dir(&stage).unwrap();
    ledger
        .append(event(
            "output_capture_prepared",
            json!({"stage":".capture-interrupted"}),
        ))
        .unwrap();
    std::fs::rename(&outputs, attempt.join(".outputs-backup")).unwrap();
    let c = Coordinator::open(
        &root.0,
        RunSettings::default(),
        Box::new(parvi_mission_engine::LabExecutor),
    )
    .unwrap();
    let blocks: Vec<_> = ledger
        .read()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "run_blocked")
        .collect();
    assert_eq!(blocks[0].data["code"], "coordinator_shutdown");
    let run = wait(&c, new_id, "succeeded");
    assert_eq!(
        std::fs::read_to_string(&run.outputs[0].value).unwrap(),
        "original"
    );
    assert!(!stage.exists());
    assert!(!attempt.join(".outputs-backup").exists());
}
