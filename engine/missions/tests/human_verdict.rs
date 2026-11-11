use parvi_mission_model::HumanVerdict;
use parvi_missions::{Coordinator, LabExecutor, Ledger, RunEvent, RunSettings};
use std::sync::{Arc, Barrier};

#[test]
fn verdicts_are_atomic_and_preserve_response() {
    let mut random = [0; 8];
    getrandom::fill(&mut random).unwrap();
    let root =
        std::env::temp_dir().join(format!("parvi-100-verdict-{}", u64::from_ne_bytes(random)));
    let coordinator =
        Coordinator::open(&root, RunSettings::default(), Box::new(LabExecutor)).unwrap();
    let ledger = Ledger::new(root.join("runs/run_test.ndjson"), "run_test");
    let event = |kind: &str| RunEvent {
        kind: kind.into(),
        actor: "agent:test".into(),
        timestamp: "2026-11-06T00:00:00Z".into(),
        ..Default::default()
    };
    ledger.create(event("run_started")).unwrap();
    let mut evaluating = event("gate_evaluating");
    evaluating.gate_id = "review".into();
    ledger.append(evaluating).unwrap();
    let mut pending = event("human_input_requested");
    pending.gate_id = "review".into();
    ledger.append(pending).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let c = coordinator.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                c.record_verdict(
                    "run_test",
                    "review",
                    HumanVerdict {
                        requested_seq: 3,
                        verdict: "pass".into(),
                        response: "  yes\n".into(),
                        relayed_by: "seat-1".into(),
                        ..Default::default()
                    },
                )
            })
        })
        .collect();
    assert_eq!(
        workers
            .into_iter()
            .map(|w| w.join().unwrap())
            .filter(Result::is_ok)
            .count(),
        1
    );
    let events = ledger.read().unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[3].data["reason"], "  yes\n");
    assert_eq!(events[3].data["relayedBy"], "seat-1");
    let error = coordinator
        .record_verdict(
            "run_test",
            "review",
            HumanVerdict {
                requested_seq: 3,
                verdict: "pass".into(),
                relayed_by: "-bad".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code, "invalid_input");
    drop(coordinator);
    std::fs::remove_dir_all(root).unwrap();
}
