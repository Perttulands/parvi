//! A run carries its work item end to end (#180): the reference on the run,
//! in run status and the run list; `{bead}` and `{issue}` in briefs with
//! Archon's precedence against declared inputs; and a brief's own work
//! item beside the run's.
use parvi_mission_engine::{
    Coordinator, ExecutionSink, Executor, FormationExecution, FormationResult, LabExecutor,
    RunSettings,
};
use parvi_mission_model::RunStart;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const BEAD: &str = "bead:n1:/work/example#ctx-12";
const ISSUE: &str = "gh:example/project#180";

/// One solo step whose brief names the run's work item every way, and
/// optionally its own; `inputs` declares the Input card's inputs.
fn mission(inputs: &str, brief_work: &str) -> String {
    format!(
        r#"schema = 1
id = "msn_work"
slug = "work"
title = "Work"
rev = 1

[[inputCard]]
id = "inp_work"
title = "Work"
goal = "Carry the work item"
{inputs}

[[formation]]
id = "fmn_work"
type = "solo"
title = "Work"
[formation.brief]
goal = "bead={{bead}} issue={{issue}} literal={{{{bead}}}}"
{brief_work}
[[formation.input]]
id = "port_in"
label = "Input"
[[formation.output]]
id = "port_out"
label = "Result"
[[formation.slot]]
id = "slot_work"
label = "Worker"
harness = "openai-codex"
agentId = "builder"
model = ""
effort = "medium"

[[end]]
id = "end_done"
title = "Done"
outcome = "done"

[[connection]]
id = "edge_start"
from = "inp_work:out"
to = "fmn_work:port_in"

[[connection]]
id = "edge_done"
from = "fmn_work:port_out"
to = "end_done:in"
"#
    )
}

struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new(mission: &str) -> Self {
        let mut b = [0; 8];
        getrandom::fill(&mut b).unwrap();
        let p = std::env::temp_dir().join(format!("parvi-180-work-{}", u64::from_ne_bytes(b)));
        std::fs::create_dir_all(p.join("missions")).unwrap();
        std::fs::write(p.join("missions/work.mission.toml"), mission).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The lab executor, keeping each step's execution as the seat would get it.
#[derive(Clone, Default)]
struct Recording(Arc<Mutex<Vec<FormationExecution>>>);
impl Executor for Recording {
    fn execute_formation(
        &self,
        execution: FormationExecution,
        events: ExecutionSink,
    ) -> Result<FormationResult, parvi_mission_engine::Error> {
        {
            self.0.lock().unwrap().push(execution.clone());
            LabExecutor
                .execute_formation(execution, events)
                .map_err(|error| error.message)
        }
        .map_err(|message: String| parvi_mission_engine::Error::new("executor_error", message))
    }
}

/// Runs the mission with `work` and `inputs`; the step's execution and the
/// run's projection once final.
fn run(
    mission: &str,
    work: &str,
    inputs: &[(&str, &str)],
) -> (
    FormationExecution,
    parvi_mission_model::RunProjection,
    String,
) {
    let dir = Scratch::new(mission);
    let recording = Recording::default();
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(recording.clone())).unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "work".into(),
            input_card_id: "inp_work".into(),
            expected_rev: 1,
            work: work.into(),
            inputs: inputs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
            ..Default::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !c.run(&id).unwrap().r#final {
        assert!(Instant::now() < deadline, "run did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    let listed = c.list(None, false).unwrap();
    assert_eq!(listed.len(), 1);
    let execution = recording.0.lock().unwrap()[0].clone();
    (execution, c.run(&id).unwrap(), listed[0].work.clone())
}

#[test]
fn a_bead_on_the_run_fills_bead_and_issue_and_is_projected() {
    let (execution, projection, listed) = run(&mission("", ""), BEAD, &[("brief", "go")]);
    assert_eq!(projection.status, "succeeded");
    assert_eq!((projection.work.as_str(), listed.as_str()), (BEAD, BEAD));
    assert_eq!(execution.work, BEAD);
    assert_eq!(
        execution.formation.brief.unwrap().goal,
        "bead=ctx-12 issue=ctx-12 literal={bead}"
    );
}

#[test]
fn a_github_issue_on_the_run_fills_the_same_placeholders() {
    let (execution, projection, _) = run(&mission("", ""), ISSUE, &[("brief", "go")]);
    assert_eq!(projection.work, ISSUE);
    assert_eq!(
        execution.formation.brief.unwrap().goal,
        "bead=example/project#180 issue=example/project#180 literal={bead}"
    );
}

#[test]
fn without_a_work_item_the_placeholders_are_empty() {
    let (execution, projection, _) = run(&mission("", ""), "", &[("brief", "go")]);
    assert_eq!(projection.work, "");
    assert_eq!(
        execution.formation.brief.unwrap().goal,
        "bead= issue= literal={bead}"
    );
}

#[test]
fn a_declared_bead_input_wins_even_when_omitted() {
    let declared =
        r#"inputs = [{ name = "bead", kind = "text" }, { name = "topic", kind = "text" }]"#;
    let (execution, _, _) = run(&mission(declared, ""), BEAD, &[("bead", "chosen")]);
    assert_eq!(
        execution.formation.brief.unwrap().goal,
        "bead=chosen issue=ctx-12 literal={bead}"
    );
    let (execution, _, _) = run(&mission(declared, ""), BEAD, &[("topic", "x")]);
    assert_eq!(
        execution.formation.brief.unwrap().goal,
        "bead=(not supplied) issue=ctx-12 literal={bead}"
    );
    let declared = r#"inputs = [{ name = "issue", kind = "text" }]"#;
    let (execution, _, _) = run(&mission(declared, ""), ISSUE, &[]);
    assert_eq!(
        execution.formation.brief.unwrap().goal,
        "bead=example/project#180 issue=(not supplied) literal={bead}"
    );
}

#[test]
fn a_brief_names_its_own_work_item_beside_the_runs() {
    for brief_work in ["ctx-9", ISSUE, BEAD] {
        let line = format!("work = \"{brief_work}\"");
        let (execution, _, _) = run(&mission("", &line), BEAD, &[("brief", "go")]);
        assert_eq!(execution.work, BEAD);
        assert_eq!(execution.formation.brief.unwrap().work, brief_work);
    }
}

#[test]
fn admission_refuses_a_work_item_it_cannot_place() {
    let dir = Scratch::new(&mission("", ""));
    let c = Coordinator::open(&dir.0, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    for work in ["ctx-12", "owner/repo", "bead:n1:relative#ctx-1"] {
        let refused = c.start_mission(RunStart {
            mission: "work".into(),
            input_card_id: "inp_work".into(),
            expected_rev: 1,
            work: work.into(),
            inputs: BTreeMap::from([("brief".into(), "go".into())]),
            ..Default::default()
        });
        assert!(
            matches!(
                refused,
                Err(parvi_mission_engine::AdmissionFailure::Invalid(_))
            ),
            "{work}"
        );
    }
}

/// Fails its first step, so the run blocks and can be resumed; records
/// every execution.
#[derive(Clone, Default)]
struct FailOnce(Recording, Arc<Mutex<bool>>);
impl Executor for FailOnce {
    fn execute_formation(
        &self,
        execution: FormationExecution,
        events: ExecutionSink,
    ) -> Result<FormationResult, parvi_mission_engine::Error> {
        (|| -> Result<FormationResult, String> {
            if !std::mem::replace(&mut *self.1.lock().unwrap(), true) {
                return Err("seat lost".into());
            }
            self.0
                .execute_formation(execution, events)
                .map_err(|error| error.message)
        })()
        .map_err(|message: String| parvi_mission_engine::Error::new("executor_error", message))
    }
}

fn open(dir: &Scratch, executor: FailOnce) -> Coordinator {
    // A dropped coordinator's worker may still hold the state lock briefly.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match Coordinator::open(&dir.0, RunSettings::default(), Box::new(executor.clone())) {
            Ok(c) => return c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("open: {e}"),
        }
    }
}

fn wait_for(c: &Coordinator, id: &str, done: impl Fn(&parvi_mission_model::RunProjection) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(&c.run(id).unwrap()) {
        assert!(Instant::now() < deadline, "run did not get there");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A ledger written before #180 named its GitHub issue `issue` on every
/// event and in `run_started`'s data. Opening the state directory rewrites
/// it whole with the canonical reference, and the run keeps its issue in
/// status, the list and a resumed step.
#[test]
fn an_old_ledgers_issue_survives_the_upgrade_and_a_resume() {
    let dir = Scratch::new(&mission("", ""));
    let executor = FailOnce::default();
    let c = open(&dir, executor.clone());
    let id = c
        .start_mission(RunStart {
            mission: "work".into(),
            input_card_id: "inp_work".into(),
            expected_rev: 1,
            work: ISSUE.into(),
            inputs: BTreeMap::from([("brief".into(), "go".into())]),
            ..Default::default()
        })
        .unwrap();
    wait_for(&c, &id, |p| p.status == "blocked");
    drop(c);

    // Write it back as the old code did.
    let path = dir.0.join("runs").join(format!("{id}.ndjson"));
    let old: String = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| {
            let mut event: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(line).unwrap();
            assert_eq!(event.remove("work").unwrap(), ISSUE);
            event.insert("issue".into(), "example/project#180".into());
            if let Some(serde_json::Value::Object(data)) = event.get_mut("data")
                && let Some(work) = data.remove("work")
            {
                assert_eq!(work, ISSUE);
                data.insert("issue".into(), "example/project#180".into());
            }
            serde_json::to_string(&event).unwrap() + "\n"
        })
        .collect();
    assert!(!old.contains(ISSUE));
    std::fs::write(&path, &old).unwrap();

    let c = open(&dir, executor.clone());
    let migrated = std::fs::read_to_string(&path).unwrap();
    assert!(!migrated.contains("\"issue\""), "{migrated}");
    assert_eq!(migrated.lines().count(), old.lines().count());
    assert_eq!(c.run(&id).unwrap().work, ISSUE);
    assert_eq!(c.list(None, false).unwrap()[0].work, ISSUE);
    // Opening again finds nothing to do.
    drop(c);
    let c = open(&dir, executor.clone());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), migrated);

    c.resume(&id, Default::default()).unwrap();
    wait_for(&c, &id, |p| p.r#final);
    assert_eq!(c.run(&id).unwrap().status, "succeeded");
    let execution = executor.0.0.lock().unwrap().last().unwrap().clone();
    assert_eq!(execution.work, ISSUE);
    assert_eq!(
        execution.formation.brief.unwrap().goal,
        "bead=example/project#180 issue=example/project#180 literal={bead}"
    );
}

/// The old admission took some issues that name no GitHub issue. A ledger
/// with one is left as it was and only that run refuses to open, naming
/// it; the coordinator and its other runs carry on.
#[test]
fn an_old_ledger_whose_issue_names_nothing_fails_alone() {
    let dir = Scratch::new(&mission("", ""));
    let executor = FailOnce::default();
    let c = open(&dir, executor.clone());
    let id = c
        .start_mission(RunStart {
            mission: "work".into(),
            input_card_id: "inp_work".into(),
            expected_rev: 1,
            work: ISSUE.into(),
            inputs: BTreeMap::from([("brief".into(), "go".into())]),
            ..Default::default()
        })
        .unwrap();
    wait_for(&c, &id, |p| p.status == "blocked");
    drop(c);

    let runs = dir.0.join("runs");
    let ledger = std::fs::read_to_string(runs.join(format!("{id}.ndjson"))).unwrap();
    let issues = [
        "example/project#0",
        "example/project#18446744073709551616",
        "-x/parvi#1",
    ];
    let old: Vec<(String, String)> = issues
        .iter()
        .enumerate()
        .map(|(n, issue)| {
            let old_id = format!("run_old{n}");
            let text = ledger.replace(&id, &old_id).replace(
                &format!("\"work\":\"{ISSUE}\""),
                &format!("\"issue\":\"{issue}\""),
            );
            assert!(!text.contains(ISSUE));
            std::fs::write(runs.join(format!("{old_id}.ndjson")), &text).unwrap();
            (old_id, text)
        })
        .collect();

    let c = open(&dir, executor);
    assert_eq!(c.run(&id).unwrap().work, ISSUE);
    assert_eq!(c.list(None, false).unwrap().len(), 1);
    for ((old_id, text), issue) in old.iter().zip(issues) {
        let refused = c.run(old_id).unwrap_err().to_string();
        assert!(refused.contains(&format!("{issue:?}")), "{refused}");
        assert!(c.resume(old_id, Default::default()).is_err());
        let kept = std::fs::read_to_string(runs.join(format!("{old_id}.ndjson"))).unwrap();
        assert_eq!(&kept, text, "left as it was");
    }
    c.resume(&id, Default::default()).unwrap();
    wait_for(&c, &id, |p| p.r#final);
}
