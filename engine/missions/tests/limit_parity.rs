//! Limit cards and Best of N, ported from Archon's `limit_card_test.go`,
//! `run_limit_fail_test.go` and `run_pick_test.go` (example/project#233).
#[allow(dead_code)]
mod run_support;
use parvi_missions::{
    Coordinator, ExecutionEvent, ExecutionSink, Executor, FormationExecution, FormationResult,
    HumanPick, HumanVerdict, LabExecutor, OpenDispatch, ResumeRequest, RunProjection, RunSettings,
    RunStart,
};
use run_support::Scratch;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    io::ErrorKind,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn formation(id: &str, title: &str, goal: &str) -> String {
    format!(
        "[[formation]]\nid = \"{id}\"\ntype = \"solo\"\ntitle = \"{title}\"\n[formation.brief]\ngoal = \"{goal}\"\n[[formation.input]]\nid = \"port_in\"\nlabel = \"Input\"\n[[formation.output]]\nid = \"port_out\"\nlabel = \"Result\"\n[[formation.slot]]\nid = \"slot_{id}\"\nlabel = \"Seat\"\nagentId = \"builder\"\nharness = \"openai-codex\"\nmodel = \"\"\neffort = \"medium\"\n\n"
    )
}

fn connection(id: &str, from: &str, to: &str) -> String {
    format!("[[connection]]\nid = \"{id}\"\nfrom = \"{from}\"\nto = \"{to}\"\n\n")
}

/// Input -> Work -> Review (pass Ship -> Done, fail back to Work), Review
/// judged by the chain Judge 1 -> Judge 2 when its kinds hold formation,
/// with the given Limit cards.
fn pick_mission(kinds: &str, work_goal: &str, cards: &str) -> String {
    let mut mission = String::from(
        "schema = 1\nid = \"msn_pick\"\nslug = \"hello\"\ntitle = \"Pick\"\nrev = 1\n\n[[inputCard]]\nid = \"inp_hello\"\ntitle = \"Hello\"\ngoal = \"Return the supplied input\"\n\n",
    );
    mission += &formation("fmn_work", "Work", work_goal);
    mission += &formation("fmn_ship", "Ship", "Ship it.");
    let judged = kinds.contains("formation");
    if judged {
        mission += &formation("fmn_j1", "Judge 1", "Review the work.");
        mission += &formation("fmn_j2", "Judge 2", "Decide.");
    }
    mission += &format!(
        "[[gate]]\nid = \"gate_review\"\ntitle = \"Review\"\nkinds = {kinds}\ncriterion = \"Judge the work\"\n\n[[end]]\nid = \"end_done\"\ntitle = \"Done\"\noutcome = \"done\"\n\n"
    );
    mission += &connection("edge_start", "inp_hello:out", "fmn_work:port_in");
    mission += &connection("edge_review", "fmn_work:port_out", "gate_review:in");
    mission += &connection("edge_pass", "gate_review:pass", "fmn_ship:port_in");
    mission += &connection("edge_fail", "gate_review:fail", "fmn_work:port_in");
    mission += &connection("edge_done", "fmn_ship:port_out", "end_done:in");
    if judged {
        mission += &connection("edge_judge", "gate_review:judge", "fmn_j1:port_in");
        mission += &connection("edge_chain", "fmn_j1:port_out", "fmn_j2:port_in");
        mission += &connection("edge_return", "fmn_j2:port_out", "gate_review:judge");
    }
    mission + cards
}

fn work_card(knobs: &str) -> String {
    format!("[[limit]]\nid = \"lim_cap\"\ntitle = \"Cap\"\ntarget = \"fmn_work\"\n{knobs}\n")
}

fn verdict(verdict: &str, reason: &str) -> String {
    format!(
        "```parvi-verdict\n{}\n```",
        json!({"verdict":verdict,"reason":reason,"evidence":[]})
    )
}

fn pick_block(attempt: i64, reason: &str, evidence: &[&str]) -> String {
    format!(
        "```parvi-pick\n{}\n```",
        json!({"attempt":attempt,"reason":reason,"evidence":evidence})
    )
}

/// One executed attempt as the scripted lab saw it.
#[derive(Clone, Debug)]
struct Call {
    node: String,
    attempt: i64,
    input: String,
    picks: Vec<i64>,
    judge: bool,
}
impl Call {
    fn trail(&self) -> String {
        format!("{}#{}", self.node, self.attempt)
    }
}

type Responses = HashMap<&'static str, BTreeMap<i64, String>>;

/// The lab's events with scripted text: each node's nth call answers
/// `responses[node][n]`; a call with no script answers the lab's own text. A
/// node in `stall` keeps working until its Limit card is spent.
#[derive(Clone, Default)]
struct Scripted {
    calls: Arc<Mutex<Vec<Call>>>,
    responses: Arc<Mutex<Responses>>,
    stall: Option<(&'static str, i64)>,
}
impl Executor for Scripted {
    fn execute_formation(
        &self,
        request: FormationExecution,
        events: ExecutionSink,
    ) -> Result<FormationResult, String> {
        let node = request.formation.id.clone();
        let attempt = request.attempt;
        self.calls.lock().unwrap().push(Call {
            node: node.clone(),
            attempt,
            input: request.input_text.clone(),
            picks: request.pick_attempts.clone(),
            judge: request.judge,
        });
        if self
            .stall
            .is_some_and(|(stall, at)| stall == node && at == attempt)
        {
            while !request.limits.is_spent() {
                std::thread::sleep(Duration::from_millis(10));
            }
            return Err("stopped at the limit".into());
        }
        let text = (self.responses.lock().unwrap().get(node.as_str()))
            .and_then(|script| script.get(&attempt).cloned());
        let mut result = LabExecutor.execute_formation(request, events)?;
        if let Some(text) = text {
            result.text.clone_from(&text);
            for output in result.outputs.values_mut() {
                output["text"] = text.clone().into();
            }
        }
        Ok(result)
    }
}
impl Scripted {
    fn new(responses: Responses) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            ..Default::default()
        }
    }
    fn trail(&self) -> Vec<String> {
        self.calls.lock().unwrap().iter().map(Call::trail).collect()
    }
    fn calls_of(&self, node: &str) -> Vec<Call> {
        (self.calls.lock().unwrap().iter())
            .filter(|call| call.node == node)
            .cloned()
            .collect()
    }
}

fn script(entries: &[(&'static str, &[(i64, &str)])]) -> Responses {
    (entries.iter())
        .map(|(node, answers)| {
            let answers = (answers.iter())
                .map(|(attempt, text)| (*attempt, (*text).to_owned()))
                .collect();
            (*node, answers)
        })
        .collect()
}

/// Work's three drafts, each judged a fail; Judge 2's fourth answer is `pick`.
fn three_failed_drafts(pick: &str) -> Responses {
    let fails = [
        verdict("fail", "no tests"),
        verdict("fail", "retry untested"),
        verdict("fail", "tests break the build"),
    ];
    script(&[
        (
            "fmn_work",
            &[(1, "draft one"), (2, "draft two"), (3, "draft three")],
        ),
        (
            "fmn_j1",
            &[
                (1, "review one"),
                (2, "review two"),
                (3, "review three"),
                (4, "compared the three drafts"),
            ],
        ),
        (
            "fmn_j2",
            &[(1, &fails[0]), (2, &fails[1]), (3, &fails[2]), (4, pick)],
        ),
        ("fmn_ship", &[(1, "shipped")]),
    ])
}

struct Run {
    root: Scratch,
    c: Coordinator,
    id: String,
}
impl Run {
    fn start(mission: &str, executor: Scripted) -> Self {
        let root = Scratch::new();
        std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
        let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(executor)).unwrap();
        let id = c
            .start_mission(RunStart {
                mission: "hello".into(),
                input_card_id: "inp_hello".into(),
                expected_rev: 1,
                inputs: BTreeMap::from([("brief".into(), "the task".into())]),
                ..Default::default()
            })
            .unwrap_or_else(|e| panic!("start: {e:?}"));
        Self { root, c, id }
    }
    /// Wait until the run is final, blocked, or waiting on a gate.
    fn settle(&self) -> RunProjection {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let p = self.c.run(&self.id).unwrap();
            if p.r#final || p.status == "blocked" || !p.waiting_gates.is_empty() {
                return p;
            }
            assert!(
                Instant::now() < deadline,
                "run did not settle:\n{}",
                self.ledger_text()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// Resume once the worker that blocked the run has let go of it.
    fn resume(&self, request: ResumeRequest) -> std::io::Result<RunProjection> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.c.resume(&self.id, request.clone()) {
                Err(e)
                    if e.kind() == ErrorKind::AlreadyExists
                        && e.to_string() == "run is working" =>
                {
                    assert!(Instant::now() < deadline, "the run's worker never stopped");
                    std::thread::sleep(Duration::from_millis(5));
                }
                result => return result,
            }
        }
    }
    fn events(&self) -> Vec<Value> {
        (self.ledger_text().lines())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn of_type(&self, kind: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| e["type"] == kind)
            .collect()
    }
    fn ledger_text(&self) -> String {
        std::fs::read_to_string(self.root.0.join("runs").join(format!("{}.ndjson", self.id)))
            .unwrap()
    }
    fn pick_verdicts(&self) -> Vec<Value> {
        (self.of_type("gate_verdict").into_iter())
            .filter(|e| e["data"].get("pick").is_some())
            .collect()
    }
}

fn resume_with(grant: bool, pick_best: bool) -> ResumeRequest {
    ResumeRequest {
        grant,
        pick_best,
        actor: "agent:test".into(),
        ..Default::default()
    }
}

fn refused(result: std::io::Result<RunProjection>, words: &str) {
    match result {
        Err(e) if e.kind() == ErrorKind::InvalidInput && e.to_string().contains(words) => {}
        other => panic!("expected a refusal naming {words:?}, got {other:?}"),
    }
}

#[test]
fn a_step_limit_stops_at_its_rounds_and_a_grant_gives_one_more() {
    let lab = Scripted::new(three_failed_drafts(""));
    let run = Run::start(
        &pick_mission("[\"formation\"]", "Draft.", &work_card("rounds = 2")),
        lab.clone(),
    );
    let p = run.settle();
    assert_eq!(
        (
            p.status.as_str(),
            p.block_code.as_str(),
            p.resume_policy.as_str()
        ),
        ("blocked", "limit_reached", "grant_or_pick")
    );
    let block = run.of_type("run_blocked").pop().unwrap();
    assert_eq!(block["nodeId"], "fmn_work");
    assert_eq!(
        block["data"]["limit"],
        json!({"kind":"rounds","limitId":"lim_cap","nodeId":"fmn_work","used":2,"max":2})
    );
    assert_eq!(block["data"]["reason"], "Work used 2 of 2 rounds");
    let error = run.of_type("error").pop().unwrap();
    assert_eq!(
        (
            error["data"]["code"].as_str(),
            error["data"]["boundary"].as_str()
        ),
        (Some("limit_reached"), Some("limits"))
    );
    refused(
        run.resume(resume_with(false, false)),
        "resume it with --grant",
    );
    assert_eq!(
        run.resume(resume_with(true, false)).unwrap().status,
        "running"
    );
    let p = run.settle();
    assert_eq!(p.block_code, "limit_reached");
    let resumed = run.of_type("run_resumed").pop().unwrap();
    assert_eq!(
        resumed["data"]["grant"],
        json!({"limitId":"lim_cap","kind":"rounds","amount":1})
    );
    let block = run.of_type("run_blocked").pop().unwrap();
    assert_eq!(block["data"]["limit"]["max"], 3);
    assert_eq!(block["data"]["limit"]["granted"], 1);
    assert_eq!(
        block["data"]["reason"],
        "Work used 3 of 3 rounds, 1 of them granted"
    );
    assert_eq!(lab.calls_of("fmn_work").len(), 3);
}

#[test]
fn a_mission_limit_stops_the_whole_run_at_its_rounds() {
    let lab = Scripted::new(three_failed_drafts(""));
    let card = "[[limit]]\nid = \"lim_all\"\ntitle = \"All\"\ntarget = \"inp_hello\"\nrounds = 4\n";
    let run = Run::start(
        &pick_mission("[\"formation\"]", "Draft.", card),
        lab.clone(),
    );
    let p = run.settle();
    assert_eq!(
        (p.block_code.as_str(), p.resume_policy.as_str()),
        ("limit_reached", "grant")
    );
    assert_eq!(
        lab.trail(),
        ["fmn_work#1", "fmn_j1#1", "fmn_j2#1", "fmn_work#2"]
    );
    let block = run.of_type("run_blocked").pop().unwrap();
    assert_eq!(block["nodeId"], "fmn_j1");
    assert_eq!(block["data"]["reason"], "The mission used 4 of 4 rounds");
    // A mission card never picks.
    refused(run.resume(resume_with(false, true)), "nothing to pick");
}

#[test]
fn a_grant_resumes_only_a_spent_limit() {
    let mut responses = three_failed_drafts("");
    responses
        .get_mut("fmn_j2")
        .unwrap()
        .insert(1, "```parvi-verdict\n{invalid}\n```".into());
    let run = Run::start(
        &pick_mission("[\"formation\"]", "Draft.", &work_card("rounds = 3")),
        Scripted::new(responses),
    );
    let p = run.settle();
    assert_eq!(p.block_code, "invalid_judge_result");
    refused(run.resume(resume_with(true, false)), "nothing to grant");
    refused(run.resume(resume_with(false, true)), "nothing to pick");
}

#[test]
fn a_fail_limit_keeps_the_judged_attempts_without_blocking() {
    let lab = Scripted::new(three_failed_drafts(""));
    let run = Run::start(
        &pick_mission(
            "[\"formation\"]",
            "Draft.",
            &work_card("rounds = 2\nwhenSpent = \"fail\""),
        ),
        lab.clone(),
    );
    let p = run.settle();
    assert!(p.r#final);
    assert_eq!(p.status, "failed");
    let failed = run.of_type("run_failed").pop().unwrap();
    assert_eq!(failed["actor"], "archond");
    assert_eq!(failed["data"]["code"], "limit_reached");
    assert_eq!(failed["data"]["limit"]["used"], 2);
    assert!(
        run.of_type("run_blocked").is_empty() && run.of_type("error").is_empty(),
        "{}",
        run.ledger_text()
    );
    let outputs = run
        .of_type("node_output")
        .into_iter()
        .filter(|e| e["nodeId"] == "fmn_work")
        .count();
    assert_eq!(outputs, 2, "the attempts stay as evidence");
}

#[test]
fn a_fail_token_limit_cuts_off_the_working_attempt() {
    let run = Run::start(
        &pick_mission(
            "[\"formation\"]",
            "Draft. archon-lab-tokens: 150",
            &work_card("tokens = 100\nwhenSpent = \"fail\""),
        ),
        Scripted::default(),
    );
    let p = run.settle();
    assert_eq!(p.status, "failed");
    let failed = run.of_type("run_failed").pop().unwrap();
    assert_eq!(
        failed["data"]["limit"],
        json!({"kind":"tokens","limitId":"lim_cap","nodeId":"fmn_work","used":150,"max":100})
    );
    assert_eq!(failed["data"]["reason"], "Work used 150 of 100 tokens");
    let usage = run.of_type("token_usage");
    assert_eq!(
        (usage.len(), usage[0]["data"]["tokens"].as_i64()),
        (1, Some(150))
    );
    assert!(
        run.of_type("node_output")
            .iter()
            .all(|e| e["nodeId"] != "fmn_work")
    );
}

#[test]
fn a_fail_time_limit_warns_then_cuts_off_the_working_attempt() {
    let run = Run::start(
        &pick_mission(
            "[\"formation\"]",
            "Draft. archon-lab-work: 6s",
            &work_card("seconds = 2\nwarnSeconds = 1\nwhenSpent = \"fail\""),
        ),
        Scripted::default(),
    );
    let p = run.settle();
    assert_eq!(p.status, "failed");
    let failed = run.of_type("run_failed").pop().unwrap();
    assert_eq!(failed["data"]["limit"]["kind"], "time");
    assert_eq!(failed["data"]["reason"], "Work used 2 s of 2 s");
    let warnings = run.of_type("limit_warning");
    assert_eq!(warnings.len(), 1, "{}", run.ledger_text());
    assert_eq!(warnings[0]["slotId"], "slot_fmn_work");
    assert!(warnings[0]["data"]["text"].as_str().unwrap().starts_with("Parvi: 1 s left of this step's working time (Limit card Cap). When it runs out the step stops and the run fails"));
}

#[test]
fn a_best_card_picks_the_best_judged_attempt_and_goes_on() {
    let lab = Scripted::new(three_failed_drafts(&pick_block(
        2,
        "the only draft whose tests pass",
        &["draft two: retry covered"],
    )));
    let run = Run::start(
        &pick_mission(
            "[\"formation\"]",
            "Draft.",
            &work_card("rounds = 3\nwhenSpent = \"best\""),
        ),
        lab.clone(),
    );
    let p = run.settle();
    assert_eq!(
        (p.status.as_str(), p.r#final),
        ("picked", true),
        "{}",
        run.ledger_text()
    );
    assert_eq!(
        lab.trail(),
        [
            "fmn_work#1",
            "fmn_j1#1",
            "fmn_j2#1",
            "fmn_work#2",
            "fmn_j1#2",
            "fmn_j2#2",
            "fmn_work#3",
            "fmn_j1#3",
            "fmn_j2#3",
            "fmn_j1#4",
            "fmn_j2#4",
            "fmn_ship#1"
        ]
    );
    assert!(run.of_type("run_blocked").is_empty() && run.of_type("error").is_empty());
    // Both judges compare every judged attempt side by side; the last reads
    // the first's recommendation below the attempts.
    let (j1, j2) = (
        lab.calls_of("fmn_j1")[3].clone(),
        lab.calls_of("fmn_j2")[3].clone(),
    );
    for call in [&j1, &j2] {
        for text in [
            "Pick the best attempt of Work",
            "criterion: Judge the work",
            "attempt 1\nverdict: fail\nreason: no tests",
            "draft one",
            "attempt 2\nverdict: fail\nreason: retry untested",
            "draft two",
            "attempt 3",
            "draft three",
        ] {
            assert!(
                call.input.contains(text),
                "{}'s pick input lacks {text:?}:\n{}",
                call.node,
                call.input
            );
        }
        assert_eq!(call.picks, [1, 2, 3]);
        assert_eq!(call.judge, call.node == "fmn_j2");
    }
    assert!(!j1.input.contains("recommendation from"));
    assert_eq!(
        j2.input,
        format!(
            "{}\nrecommendation from Judge 1:\ncompared the three drafts\n",
            j1.input
        )
    );
    let started = run.of_type("pick_started");
    assert_eq!(started.len(), 1);
    let started = &started[0];
    assert_eq!(
        (started["nodeId"].as_str(), started["gateId"].as_str()),
        (Some("fmn_work"), Some("gate_review"))
    );
    assert_eq!(started["data"]["trigger"], "whenSpent");
    assert_eq!(started["data"]["judgeChain"], json!(["fmn_j1", "fmn_j2"]));
    assert_eq!(
        started["data"]["limit"],
        json!({"kind":"rounds","limitId":"lim_cap","nodeId":"fmn_work","used":3,"max":3})
    );
    assert_eq!(started["data"]["candidates"].as_array().unwrap().len(), 3);
    let verdicts = run.pick_verdicts();
    assert_eq!(verdicts.len(), 1);
    let (verdict, pick) = (&verdicts[0]["data"], &verdicts[0]["data"]["pick"]);
    assert_eq!(
        (verdict["verdict"].as_str(), verdict["routePort"].as_str()),
        (Some("pass"), Some("pass"))
    );
    assert_eq!(verdict["reason"], "the only draft whose tests pass");
    assert_eq!(
        verdict["evidence"],
        json!([{"kind":"formation","text":"draft two: retry covered"}])
    );
    assert_eq!(
        (pick["attempt"].as_i64(), pick["decidedBy"].as_str()),
        (Some(2), Some("fmn_j2"))
    );
    assert_eq!(pick["candidates"], json!([1, 2, 3]));
    assert_eq!(pick["pickSeq"], started["seq"]);
    // Ship runs on the picked draft, with nothing marking it as picked.
    let ship = &lab.calls_of("fmn_ship")[0];
    assert_eq!(ship.input, "draft two");
    let picked = run.of_type("run_picked").pop().unwrap();
    assert_eq!(picked["data"]["picks"][0]["attempt"], 2);
    // The attempts evidence joins each judge's verdict to the draft it
    // judged and marks the draft the judge picked (#219).
    let attempts = run.c.attempts(&run.id, None).unwrap();
    let work = &attempts.steps[0];
    assert_eq!(work.node_id, "fmn_work");
    let judged: Vec<_> = (work.attempts.iter())
        .map(|a| {
            let v = &a.verdicts[0];
            (
                a.attempt,
                a.outputs[0].text.text.as_str(),
                v.verdict.as_str(),
                v.reason.text.as_str(),
            )
        })
        .collect();
    assert_eq!(
        judged,
        [
            (1, "draft one", "fail", "no tests"),
            (2, "draft two", "fail", "retry untested"),
            (3, "draft three", "fail", "tests break the build"),
        ]
    );
    let pick = work.attempts[1].picked.as_ref().expect("draft two picked");
    assert_eq!(
        (pick.decided_by.as_str(), pick.reason.text.as_str()),
        ("fmn_j2", "the only draft whose tests pass")
    );
    assert_eq!(
        (pick.candidates.as_slice(), pick.trigger.as_str()),
        (&[1, 2, 3][..], "whenSpent")
    );
    assert!(work.attempts[0].picked.is_none() && work.attempts[2].picked.is_none());
    let judge = (attempts.steps.iter())
        .find(|s| s.node_id == "fmn_j1")
        .unwrap();
    assert_eq!(
        (judge.judge_of.as_slice(), judge.attempts.len()),
        (&["gate_review".to_string()][..], 4)
    );
    let only = run.c.attempts(&run.id, Some("fmn_ship")).unwrap();
    assert_eq!(only.steps.len(), 1);
    assert_eq!(only.steps[0].attempts[0].status, "done");
    let missing = run.c.attempts(&run.id, Some("fmn_nowhere")).unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::NotFound);
}

/// A waiting human gate quotes the output that feeds it, and the attempts
/// show each verdict the operator gave (#219).
#[test]
fn a_human_gate_quotes_its_input_and_the_attempts_keep_each_answer() {
    let lab = Scripted::new(three_failed_drafts(""));
    let run = Run::start(
        &pick_mission(
            "[\"human\"]",
            "Draft.",
            &work_card("rounds = 2\nwhenSpent = \"best\""),
        ),
        lab,
    );
    for (n, draft) in [(1, "draft one"), (2, "draft two")] {
        let p = run.settle();
        let seq = p.waiting_gates[0].requested_seq;
        let request = run.c.gate_request(&run.id, "gate_review").unwrap();
        assert_eq!(
            (request.requested_seq, request.criterion.as_str()),
            (seq, "Judge the work")
        );
        assert_eq!(
            (
                request.input.from_node_id.as_str(),
                request.input.from_port_id.as_str(),
                request.input.text.as_str(),
                request.input.truncated
            ),
            ("fmn_work", "port_out", draft, false)
        );
        let work = &run.c.attempts(&run.id, Some("fmn_work")).unwrap().steps[0];
        let asked = &work.attempts[n - 1].verdicts[0];
        assert!(asked.pending && asked.verdict.is_empty(), "{asked:?}");
        run.c
            .record_verdict(
                &run.id,
                "gate_review",
                HumanVerdict {
                    requested_seq: seq,
                    verdict: "fail".into(),
                    response: format!("not yet {n}"),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let p = run.settle();
    let request = run.c.gate_request(&run.id, "gate_review").unwrap();
    assert!(request.pick.is_some(), "the pick request is served");
    run.c
        .record_pick(
            &run.id,
            "gate_review",
            HumanPick {
                requested_seq: p.waiting_gates[0].requested_seq,
                attempt: 1,
                reason: "the first reads better".into(),
                actor: "human:operator".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(run.settle().status, "picked", "{}", run.ledger_text());
    assert!(matches!(
        run.c.gate_request(&run.id, "gate_review"),
        Err(parvi_missions::GateRequestError::NotPending)
    ));
    assert!(matches!(
        run.c.gate_request(&run.id, "gate_nowhere"),
        Err(parvi_missions::GateRequestError::NotFound(_))
    ));
    let work = &run.c.attempts(&run.id, Some("fmn_work")).unwrap().steps[0];
    let answers: Vec<_> = (work.attempts.iter())
        .map(|a| {
            let v = &a.verdicts[0];
            (
                v.verdict.as_str(),
                v.reason.text.as_str(),
                v.decided_by.as_str(),
                v.pending,
            )
        })
        .collect();
    assert_eq!(
        answers,
        [
            ("fail", "not yet 1", "human:operator", false),
            ("fail", "not yet 2", "human:operator", false)
        ]
    );
    let pick = work.attempts[0].picked.as_ref().expect("the first picked");
    assert_eq!(pick.decided_by, "human:operator");
}

#[test]
fn pick_best_resumes_a_spent_limit_block() {
    let mut responses = three_failed_drafts("");
    responses
        .get_mut("fmn_j1")
        .unwrap()
        .insert(3, "compared the two drafts".into());
    responses
        .get_mut("fmn_j2")
        .unwrap()
        .insert(3, pick_block(1, "smaller diff", &[]));
    let lab = Scripted::new(responses);
    let run = Run::start(
        &pick_mission("[\"formation\"]", "Draft.", &work_card("rounds = 2")),
        lab.clone(),
    );
    let p = run.settle();
    assert_eq!(
        (p.status.as_str(), p.resume_policy.as_str()),
        ("blocked", "grant_or_pick")
    );
    refused(
        run.resume(resume_with(false, false)),
        "--pick-best to take the best attempt",
    );
    refused(run.resume(resume_with(true, true)), "not both");
    run.resume(resume_with(false, true)).unwrap();
    let p = run.settle();
    assert_eq!(p.status, "picked", "{}", run.ledger_text());
    assert_eq!(
        lab.trail(),
        [
            "fmn_work#1",
            "fmn_j1#1",
            "fmn_j2#1",
            "fmn_work#2",
            "fmn_j1#2",
            "fmn_j2#2",
            "fmn_j1#3",
            "fmn_j2#3",
            "fmn_ship#1"
        ]
    );
    assert_eq!(lab.calls_of("fmn_ship")[0].input, "draft one");
    assert_eq!(run.of_type("run_resumed")[0]["data"]["pickBest"], true);
    let pick = &run.pick_verdicts()[0]["data"]["pick"];
    assert_eq!(
        (pick["trigger"].as_str(), pick["attempt"].as_i64()),
        (Some("resume"), Some(1))
    );
    assert_eq!(pick["candidates"], json!([1, 2]));
}

#[test]
fn the_only_judged_attempt_is_taken_without_a_picker() {
    let lab = Scripted::new(three_failed_drafts(""));
    let run = Run::start(
        &pick_mission(
            "[\"formation\"]",
            "Draft.",
            &work_card("rounds = 1\nwhenSpent = \"best\""),
        ),
        lab.clone(),
    );
    assert_eq!(run.settle().status, "picked", "{}", run.ledger_text());
    assert_eq!(
        lab.trail(),
        ["fmn_work#1", "fmn_j1#1", "fmn_j2#1", "fmn_ship#1"]
    );
    let verdict = &run.pick_verdicts()[0]["data"];
    assert_eq!(verdict["reason"], "the only attempt Review judged");
    assert_eq!(verdict["pick"]["decidedBy"], "parvi");
}

#[test]
fn an_attempt_the_limit_cut_is_never_a_candidate() {
    let mut responses = three_failed_drafts("");
    responses
        .get_mut("fmn_j1")
        .unwrap()
        .insert(3, "compared the two drafts".into());
    responses
        .get_mut("fmn_j2")
        .unwrap()
        .insert(3, pick_block(2, "covers retry", &[]));
    let lab = Scripted {
        stall: Some(("fmn_work", 3)),
        ..Scripted::new(responses)
    };
    let run = Run::start(
        &pick_mission(
            "[\"formation\"]",
            "Draft.",
            &work_card("seconds = 2\nwhenSpent = \"best\""),
        ),
        lab.clone(),
    );
    assert_eq!(run.settle().status, "picked", "{}", run.ledger_text());
    assert_eq!(lab.calls_of("fmn_j2")[2].picks, [1, 2]);
    assert_eq!(lab.calls_of("fmn_ship")[0].input, "draft two");
    let error = run.of_type("error").pop().unwrap();
    assert_eq!(
        (
            error["nodeId"].as_str(),
            error["data"]["limit"]["kind"].as_str()
        ),
        (Some("fmn_work"), Some("time"))
    );
    let started = &run.of_type("pick_started")[0];
    assert_eq!(started["data"]["limit"]["kind"], "time");
}

#[test]
fn an_operator_picks_at_a_human_gate() {
    let lab = Scripted::new(three_failed_drafts(""));
    let run = Run::start(
        &pick_mission(
            "[\"human\"]",
            "Draft.",
            &work_card("rounds = 2\nwhenSpent = \"best\""),
        ),
        lab.clone(),
    );
    for _ in 0..2 {
        let p = run.settle();
        let gate = &p.waiting_gates[0];
        assert!(gate.pick.is_none(), "a verdict request is no pick");
        run.c
            .record_verdict(
                &run.id,
                "gate_review",
                HumanVerdict {
                    requested_seq: gate.requested_seq,
                    verdict: "fail".into(),
                    response: "not yet".into(),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let p = run.settle();
    let request = run.of_type("human_input_requested").pop().unwrap();
    assert_eq!(request["data"]["pick"]["candidates"], json!([1, 2]));
    let seq = p.waiting_gates[0].requested_seq;
    assert_eq!(
        p.waiting_gates[0]
            .pick
            .as_ref()
            .map(|pick| &pick["candidates"]),
        Some(&json!([1, 2])),
        "the waiting gate says it waits for a pick"
    );
    refused(
        run.c.record_verdict(
            &run.id,
            "gate_review",
            HumanVerdict {
                requested_seq: seq,
                verdict: "pass".into(),
                ..Default::default()
            },
        ),
        "answer it with gate pick",
    );
    refused(
        run.c.record_pick(
            &run.id,
            "gate_review",
            HumanPick {
                requested_seq: seq,
                attempt: 3,
                ..Default::default()
            },
        ),
        "pick one of 1 or 2",
    );
    run.c
        .record_pick(
            &run.id,
            "gate_review",
            HumanPick {
                requested_seq: seq,
                attempt: 1,
                reason: "the first reads better".into(),
                actor: "human:operator".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(run.settle().status, "picked", "{}", run.ledger_text());
    let verdict = &run.pick_verdicts()[0];
    assert_eq!(verdict["data"]["requestedSeq"].as_u64(), Some(seq));
    assert_eq!(verdict["data"]["pick"]["decidedBy"], "human:operator");
    assert_eq!(verdict["data"]["reason"], "the first reads better");
    assert_eq!(lab.calls_of("fmn_ship")[0].input, "draft one");
}

#[test]
fn a_malformed_judge_pick_blocks_and_a_resume_asks_again() {
    let mut responses = three_failed_drafts("I like the second one");
    responses
        .get_mut("fmn_j2")
        .unwrap()
        .insert(5, pick_block(3, "the last is complete", &[]));
    let lab = Scripted::new(responses);
    let run = Run::start(
        &pick_mission(
            "[\"formation\"]",
            "Draft.",
            &work_card("rounds = 3\nwhenSpent = \"best\""),
        ),
        lab.clone(),
    );
    let p = run.settle();
    assert_eq!(
        (p.status.as_str(), p.block_code.as_str()),
        ("blocked", "invalid_judge_pick"),
        "{}",
        run.ledger_text()
    );
    let failed = run.of_type("judge_attempt_failed").pop().unwrap();
    assert_eq!(failed["data"]["code"], "invalid_judge_pick");
    run.resume(resume_with(false, false)).unwrap();
    assert_eq!(run.settle().status, "picked", "{}", run.ledger_text());
    // Judge 1's recommendation is kept; only the final judge is asked again.
    assert_eq!(lab.calls_of("fmn_j1").len(), 4);
    assert_eq!(lab.calls_of("fmn_j2").len(), 5);
    assert_eq!(lab.calls_of("fmn_ship")[0].input, "draft three");
}

#[test]
fn a_resume_field_the_node_does_not_know_is_refused() {
    let known: Result<ResumeRequest, _> = serde_json::from_str(
        r#"{"grant":true,"pickBest":false,"reason":"one more","actor":"human:operator","mode":"reattach","takeFail":false}"#,
    );
    assert!(known.is_ok());
    let unknown: Result<ResumeRequest, _> =
        serde_json::from_str(r#"{"grant":true,"completedDispatchId":"x"}"#);
    assert!(
        unknown
            .unwrap_err()
            .to_string()
            .contains("unknown field `completedDispatchId`")
    );
}

/// Work's first attempt on a node that goes down while its seat works: the
/// attempt records its dispatch, and the test cuts the ledger back to it as a
/// crash leaves it. Reattached after the restart, the seat has spent `tokens`
/// and works on until its Limit card is spent.
#[derive(Clone, Default)]
struct Lost {
    tokens: i64,
    cut_after: Arc<Mutex<Option<Duration>>>,
}
impl Executor for Lost {
    fn execute_formation(
        &self,
        request: FormationExecution,
        events: ExecutionSink,
    ) -> Result<FormationResult, String> {
        if request.formation.id != "fmn_work" || request.attempt != 1 {
            return LabExecutor.execute_formation(request, events);
        }
        let slot = request.formation.slots[0].id.clone();
        events.emit(ExecutionEvent {
            kind: "slot_dispatch".into(),
            slot_id: slot.clone(),
            data: json!({"dispatchId":"dsp_lost","nodeId":"fmn_work","slotId":slot})
                .as_object()
                .unwrap()
                .clone(),
        })?;
        Err("the node went down".into())
    }
    fn reattach(&self, dispatch: OpenDispatch) -> Result<FormationResult, String> {
        let began = Instant::now();
        if self.tokens > 0 {
            dispatch
                .limits
                .count_tokens(&dispatch.dispatch_id, self.tokens);
        }
        while !dispatch.limits.is_spent() {
            if began.elapsed() > Duration::from_secs(15) {
                return Err("the Limit card never cut the seat off".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        *self.cut_after.lock().unwrap() = Some(began.elapsed());
        Err("stopped at the limit".into())
    }
}

/// Start the mission, cut its ledger back to Work's open dispatch with the
/// attempt begun `worked` before it, and restart the node on that ledger.
fn restarted_at_dispatch(mission: &str, lost: Lost, worked: Duration) -> Run {
    let root = Scratch::new();
    std::fs::write(root.0.join("missions/hello.mission.toml"), mission).unwrap();
    let c = Coordinator::open(&root.0, RunSettings::default(), Box::new(lost.clone())).unwrap();
    let id = c
        .start_mission(RunStart {
            mission: "hello".into(),
            input_card_id: "inp_hello".into(),
            expected_rev: 1,
            inputs: BTreeMap::from([("brief".into(), "the task".into())]),
            ..Default::default()
        })
        .unwrap();
    let run = Run { root, c, id };
    assert_eq!(run.settle().status, "blocked", "{}", run.ledger_text());
    let Run { root, c, id } = run;
    drop(c);
    let path = root.0.join("runs").join(format!("{id}.ndjson"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let event = |line: &str| serde_json::from_str::<Value>(line).unwrap();
    let dispatch = (lines.iter())
        .position(|line| event(line)["type"] == "slot_dispatch")
        .unwrap();
    lines.truncate(dispatch + 1);
    let started = (lines.iter())
        .rposition(|line| {
            event(line)["type"] == "node_started" && event(line)["nodeId"] == "fmn_work"
        })
        .unwrap();
    let ts = event(&lines[started])["ts"].as_str().unwrap().to_owned();
    let rfc3339 = &::time::format_description::well_known::Rfc3339;
    let earlier = ::time::OffsetDateTime::parse(&ts, rfc3339).unwrap() - worked;
    lines[started] = lines[started].replace(&ts, &earlier.format(rfc3339).unwrap());
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        match Coordinator::open(&root.0, RunSettings::default(), Box::new(lost.clone())) {
            Ok(c) => break c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("restart: {e}"),
        }
    };
    Run { root, c, id }
}

impl Run {
    fn abandoned(&self, dispatch: &str) -> bool {
        (self.of_type("slot_result").iter())
            .any(|e| e["data"]["dispatchId"] == dispatch && e["data"]["status"] == "abandoned")
    }
}

#[test]
fn a_seat_reattached_after_a_restart_is_cut_off_by_the_time_it_worked_before() {
    let lost = Lost::default();
    let run = restarted_at_dispatch(
        &pick_mission(
            "[\"formation\"]",
            "Draft.",
            &work_card("seconds = 5\nwhenSpent = \"fail\""),
        ),
        lost.clone(),
        Duration::from_secs(10),
    );
    let p = run.settle();
    assert_eq!(p.status, "failed", "{}", run.ledger_text());
    let resumed = run.of_type("run_resumed").pop().unwrap();
    assert_eq!(resumed["actor"], "parvi:restart");
    let cut = lost
        .cut_after
        .lock()
        .unwrap()
        .expect("the seat was reattached");
    assert!(
        cut < Duration::from_secs(3),
        "the reattached seat got its card's whole time again: cut off after {cut:?}"
    );
    let failed = run.of_type("run_failed").pop().unwrap();
    assert_eq!(failed["data"]["limit"]["kind"], "time");
    assert_eq!(failed["data"]["reason"], "Work used 5 s of 5 s");
    assert!(run.abandoned("dsp_lost"), "{}", run.ledger_text());
    assert!(
        run.of_type("node_output")
            .iter()
            .all(|e| e["nodeId"] != "fmn_work")
    );
}

#[test]
fn a_seat_reattached_after_a_restart_is_cut_off_at_its_tokens() {
    let run = restarted_at_dispatch(
        &pick_mission("[\"formation\"]", "Draft.", &work_card("tokens = 100")),
        Lost {
            tokens: 150,
            ..Default::default()
        },
        Duration::ZERO,
    );
    let p = run.settle();
    assert_eq!(
        (p.status.as_str(), p.block_code.as_str()),
        ("blocked", "limit_reached"),
        "{}",
        run.ledger_text()
    );
    let usage = run.of_type("token_usage");
    assert_eq!(usage.len(), 1, "{}", run.ledger_text());
    assert_eq!(
        (
            usage[0]["data"]["dispatchId"].as_str(),
            usage[0]["data"]["tokens"].as_i64()
        ),
        (Some("dsp_lost"), Some(150))
    );
    let error = (run.of_type("error").into_iter())
        .find(|e| e["data"]["code"] == "limit_reached")
        .unwrap();
    assert_eq!(
        error["data"]["limit"],
        json!({"kind":"tokens","limitId":"lim_cap","nodeId":"fmn_work","used":150,"max":100})
    );
    assert!(run.abandoned("dsp_lost"), "{}", run.ledger_text());
}
