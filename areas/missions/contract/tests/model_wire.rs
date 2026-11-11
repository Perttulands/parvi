//! Each type reads back what it writes, so the node and its clients can
//! share one declaration. Responses omit empty fields when written; requests
//! are written whole.

use parvi_missions_contract::{
    Code, Escalation, GateRequest, HumanPick, HumanVerdict, MissionListing, Report, ResumeMode,
    ResumeRequest, RunAttempts, RunProjection, RunStart, RunSummary,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn reads_back<T: Serialize + DeserializeOwned>(written: Value) -> T {
    let read: T = serde_json::from_value(written.clone()).unwrap();
    assert_eq!(serde_json::to_value(&read).unwrap(), written);
    read
}

#[test]
fn responses_read_back_with_their_empty_fields_left_out() {
    let run = reads_back::<RunProjection>(json!({
        "runId": "run_a", "status": "waiting_human", "final": false,
        "resumeAllowed": false, "epoch": 1, "missionSlug": "hello",
        "missionId": "msn_a", "missionRev": 2, "inputCardId": "inp_a",
        "work": "", "cwd": "/work", "repository": "", "base": "",
        "contextPaths": [], "inputs": [{"name": "brief", "kind": "text", "value": "hi"}],
        "outputs": [{"name": "plan", "kind": "text", "value": "done", "nodeId": "fmn_a",
                     "portId": "out", "attempt": 1}],
        "eventCount": 2, "startedBy": "human:operator",
        "startedAt": "2026-11-11T00:00:00Z", "updatedAt": "2026-11-11T00:00:01Z",
        "waitingGates": [{"gateId": "gte_a", "requestedSeq": 2,
                          "requestedAt": "2026-11-11T00:00:01Z"}],
        "events": [
            {"seq": 1, "type": "run_started"},
            {"seq": 2, "type": "human_input_requested", "gateId": "gte_a", "attempt": 1,
             "inputRefs": [{"edgeId": "edg_a", "fromNodeId": "fmn_a", "fromPortId": "out",
                            "toPortId": "in", "origin": null}]}
        ]
    }));
    assert_eq!(run.events[1].gate_id, "gte_a");

    reads_back::<Vec<RunSummary>>(json!([{
        "runId": "run_a", "number": 1, "status": "blocked", "final": false,
        "missionSlug": "hello", "missionId": "msn_a", "missionRev": 2,
        "inputCardId": "inp_a", "work": "", "eventCount": 3, "resumeAllowed": true,
        "blockCode": "limit_spent", "startedBy": "human:operator",
        "startedAt": "2026-11-11T00:00:00Z", "updatedAt": "2026-11-11T00:00:01Z",
        "inputExcerpt": "hi", "outputs": [],
        "sessions": [{"sessionName": "parvi-a", "nodeId": "fmn_a", "slotId": "w1"}]
    }]));
    reads_back::<RunAttempts>(json!({"runId": "run_a", "steps": [{
        "nodeId": "fmn_a", "title": "Plan",
        "outputs": [{"id": "out", "label": "Plan", "kind": "text"}],
        "attempts": [{
            "attempt": 1, "status": "done", "startedSeq": 3,
            "startedAt": "2026-11-11T00:00:00Z", "outputSeq": 5,
            "outputs": [{"portId": "out", "kind": "text", "text": {"text": "plan", "bytes": 4}}],
            "verdicts": [{"gateId": "gte_a", "evaluationSeq": 6, "pending": true,
                          "reason": {"text": "", "bytes": 0}, "evidence": []}]
        }]
    }]}));
    reads_back::<GateRequest>(json!({
        "gateId": "gte_a", "requestedSeq": 2, "criterion": "Is it right?",
        "input": {"text": "plan", "truncated": false}
    }));
    reads_back::<Vec<Escalation>>(json!([{
        "runId": "run_a", "seq": 4, "severity": "high", "reason": "stuck",
        "source": "agent", "trigger": "sentinel", "blocks": true
    }]));
    reads_back::<Vec<MissionListing>>(json!([{"slug": "broken", "broken": "dangling link"}]));
}

#[test]
fn findings_read_back_by_their_codes() {
    let report = reads_back::<Report>(json!({
        "errors": [
            {"code": "missing_input", "nodeId": "inp_a", "message": "brief is required"},
            {"code": "missing_file", "nodeId": "fmn_a", "message": "no such file",
             "path": "/work/brief.md"}
        ],
        "warnings": []
    }));
    assert_eq!(report.errors[1].code, Code::MissingFile);
}

#[test]
fn requests_read_back_whole() {
    let start = reads_back::<RunStart>(json!({
        "mission": "hello", "inputCardId": "inp_a", "formationId": "",
        "expectedRev": 2, "expectedEtag": "abc", "cwd": "/work", "repository": "",
        "base": "", "contextPaths": [], "work": "", "actor": "human:operator",
        "inputs": {"brief": "hi"}, "requestId": "req_1"
    }));
    assert_eq!(start.known_devices, None);
    let resume = reads_back::<ResumeRequest>(json!({
        "mode": "redispatch", "takeFail": false, "grant": true, "pickBest": false,
        "reason": "one more round", "actor": "human:operator"
    }));
    assert_eq!(resume.mode, ResumeMode::Redispatch);
    let verdict = reads_back::<HumanVerdict>(json!({
        "requestedSeq": 2, "verdict": "pass", "reason": "fine", "relayedBy": "",
        "actor": "human:operator"
    }));
    assert_eq!(verdict.response, "fine");
    reads_back::<HumanPick>(json!({
        "requestedSeq": 2, "attempt": 1, "reason": "best", "relayedBy": "w1",
        "actor": "human:operator"
    }));
}
