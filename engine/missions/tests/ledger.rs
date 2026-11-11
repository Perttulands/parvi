use parvi_missions::{Ledger, RunEvent, validate_ledger};
use std::sync::Arc;

fn event(kind: &str) -> RunEvent {
    RunEvent {
        kind: kind.into(),
        actor: "agent:test".into(),
        timestamp: "2026-11-06T00:00:00Z".into(),
        ..Default::default()
    }
}
fn scratch() -> std::path::PathBuf {
    let mut random = [0; 8];
    getrandom::fill(&mut random).unwrap();
    std::env::temp_dir().join(format!("parvi-99-ledger-{}", u64::from_ne_bytes(random)))
}
#[test]
fn concurrent_appends_are_ordered_and_final_runs_refuse_changes() {
    let path = scratch();
    let ledger = Arc::new(Ledger::new(&path, "run_test"));
    ledger.create(event("run_started")).unwrap();
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let ledger = ledger.clone();
            std::thread::spawn(move || {
                for _ in 0..5 {
                    ledger.append(event("node_started")).unwrap();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let events = ledger.read().unwrap();
    assert_eq!(events.len(), 41);
    validate_ledger(&events, "run_test").unwrap();
    ledger.append(event("run_succeeded")).unwrap();
    assert!(ledger.append(event("node_started")).is_err());
    std::fs::remove_file(path).unwrap();
}
#[test]
fn a_truncated_or_wrong_identity_ledger_is_never_projected() {
    let path = scratch();
    let ledger = Ledger::new(&path, "run_test");
    ledger.create(event("run_started")).unwrap();
    assert!(Ledger::new(&path, "run_other").read().is_err());
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"seq\":2")
        .unwrap();
    assert!(ledger.read().is_err());
    assert!(ledger.append(event("node_started")).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn projection_sanitizes_private_data_and_tracks_human_requests() {
    let path = scratch();
    let ledger = Ledger::new(&path, "run_test");
    let mut start = event("run_started");
    start.data.insert("cwd".into(), "/work".into());
    start.data.insert("prompt".into(), "secret prompt".into());
    ledger.create(start).unwrap();
    let mut request = event("human_input_requested");
    request.gate_id = "gate_review".into();
    request.data.insert("briefPath".into(), "/private".into());
    ledger.append(request).unwrap();
    let projected = parvi_missions::project_run(&ledger.read().unwrap(), "run_test").unwrap();
    assert_eq!(projected.status, "waiting_human");
    assert_eq!(projected.waiting_gates[0].requested_seq, 2);
    let json = serde_json::to_string(&projected).unwrap();
    assert!(!json.contains("secret prompt"));
    assert!(!json.contains("/private"));
    assert!(json.contains("/work"));
    ledger.append(event("run_canceled")).unwrap();
    let projected = parvi_missions::project_run(&ledger.read().unwrap(), "run_test").unwrap();
    assert!(projected.r#final);
    assert_eq!(projected.status, "canceled");
    assert!(projected.waiting_gates.is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn blocked_epoch_accepts_only_resume_cancel_failure_and_cleanup() {
    let path = scratch();
    let ledger = Ledger::new(&path, "run_test");
    ledger.create(event("run_started")).unwrap();
    let mut blocked = event("run_blocked");
    blocked.data.insert("resumeAllowed".into(), true.into());
    ledger.append(blocked).unwrap();
    assert!(ledger.append(event("node_started")).is_err());
    assert_eq!(ledger.append(event("run_resumed")).unwrap().epoch, 1);
    assert!(ledger.append(event("run_resumed")).is_err());
    ledger.append(event("node_started")).unwrap();
    std::fs::remove_file(path).unwrap();
}

#[test]
fn escalation_evidence_keeps_reason_out_of_the_public_event() {
    let path = scratch();
    let ledger = Ledger::new(&path, "run_test");
    ledger.create(event("run_started")).unwrap();
    let mut escalation = event("escalation_raised");
    escalation.node_id = "step".into();
    escalation
        .data
        .insert("reason".into(), "private reason".into());
    escalation.data.insert("severity".into(), "stop".into());
    escalation.data.insert("blocks".into(), true.into());
    ledger.append(escalation).unwrap();
    let events = ledger.read().unwrap();
    let view = parvi_missions::project_run(&events, "run_test").unwrap();
    assert!(
        !serde_json::to_string(&view)
            .unwrap()
            .contains("private reason")
    );
    let evidence = parvi_missions::project_escalations(&events);
    assert_eq!(evidence[0].reason, "private reason");
    assert!(evidence[0].blocks);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_verdict_for_an_old_request_does_not_close_the_new_one() {
    let path = scratch();
    let ledger = Ledger::new(&path, "run_test");
    ledger.create(event("run_started")).unwrap();
    let mut request = event("human_input_requested");
    request.gate_id = "review".into();
    ledger.append(request.clone()).unwrap();
    let latest = ledger.append(request).unwrap();
    let mut stale = event("human_verdict_recorded");
    stale.gate_id = "review".into();
    stale.data.insert("requestedSeq".into(), 2.into());
    ledger.append(stale).unwrap();
    let projection = parvi_missions::project_run(&ledger.read().unwrap(), "run_test").unwrap();
    assert_eq!(projection.waiting_gates[0].requested_seq, latest.seq);
    assert_eq!(projection.status, "waiting_human");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_new_evaluation_supersedes_an_older_human_request_immediately() {
    let path = scratch();
    let ledger = Ledger::new(&path, "run_test");
    ledger.create(event("run_started")).unwrap();
    let mut request = event("human_input_requested");
    request.gate_id = "review".into();
    ledger.append(request).unwrap();
    let mut evaluation = event("gate_evaluating");
    evaluation.gate_id = "review".into();
    ledger.append(evaluation).unwrap();
    let projection = parvi_missions::project_run(&ledger.read().unwrap(), "run_test").unwrap();
    assert!(projection.waiting_gates.is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn oversized_events_are_rejected_without_damaging_the_ledger() {
    let path = scratch();
    let ledger = Ledger::new(&path, "run_test");
    ledger.create(event("run_started")).unwrap();
    let mut oversized = event("node_started");
    oversized
        .data
        .insert("text".into(), "x".repeat(1 << 20).into());
    assert!(ledger.append(oversized).is_err());
    assert_eq!(ledger.read().unwrap().len(), 1);
    std::fs::remove_file(path).unwrap();
}
