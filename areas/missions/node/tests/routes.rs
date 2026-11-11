use axum::http::Method;
use parvi_node_kit::{Listing, NodeClient, NodeError};
use parvi_testkit::{node::Node, sessions};
use serde_json::json;

const AREAS: &[Listing] = &[
    sessions::LISTING,
    parvi_work::LISTING,
    parvi_missions::LISTING,
];

#[tokio::test(flavor = "multi_thread")]
async fn mission_routes_on_a_node_with_fake_sessions() {
    let node = Node::start(AREAS, "").await.unwrap();
    let client = NodeClient::new(Some(&format!("{}/", node.url()))).unwrap();
    let (created, etag) = client
        .request(
            Method::POST,
            "/api/missions",
            Some(json!({"title":"Example"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(created["mission"]["title"], "Example");
    let (listed, _) = client
        .request(Method::GET, "/api/missions", None, None)
        .await
        .unwrap();
    assert_eq!(listed["missions"][0]["slug"], "example");
    let (edited, _) = client
        .request(
            Method::PATCH,
            "/api/missions/example",
            Some(json!({"expectedRev":1,"title":"Renamed"})),
            Some(&etag),
        )
        .await
        .unwrap();
    assert_eq!(edited["mission"]["title"], "Renamed");
    let (notes, notes_etag) = client
        .request(Method::GET, "/api/missions/example/notes", None, None)
        .await
        .unwrap();
    assert!(notes["notes"]["mission"].is_array());
    let (updated_notes, _) = client
        .request(
            Method::PATCH,
            "/api/missions/example/notes",
            Some(json!({"target":"mission","text":"intent","author":"human:test"})),
            Some(&notes_etag),
        )
        .await
        .unwrap();
    assert_eq!(updated_notes["notes"]["mission"][0]["text"], "intent");
    let (validation, _) = client
        .request(Method::GET, "/api/missions/example/validation", None, None)
        .await
        .unwrap();
    assert!(validation["errors"].is_array());
    let (profiles, _) = client
        .request(Method::GET, "/api/gate-profiles", None, None)
        .await
        .unwrap();
    assert!(!profiles["profiles"].as_array().unwrap().is_empty());
    let (characters, _) = client
        .request(Method::GET, "/api/characters", None, None)
        .await
        .unwrap();
    assert!(characters["characters"].is_array());
    let (character, _) = client
        .request(
            Method::POST,
            "/api/characters",
            Some(json!({"name":"Reviewer","harness":"claude-code","effort":"medium"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(character["id"], "reviewer");
    let (updated, _) = client
        .request(
            Method::PUT,
            "/api/characters/reviewer",
            Some(json!({"name":"Reviewer","harness":"claude-code","effort":"high"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(updated["effort"], "high");
    let (removed, _) = client
        .request(Method::DELETE, "/api/characters/reviewer", None, None)
        .await
        .unwrap();
    assert_eq!(removed["deleted"], "reviewer");
    let (layout, layout_etag) = client
        .request(Method::GET, "/api/missions/example/layout", None, None)
        .await
        .unwrap();
    assert!(layout["layout"]["nodes"].is_array());
    let (changed_layout, _) = client
        .request(
            Method::PATCH,
            "/api/missions/example/layout",
            Some(json!({"nodes":[{"id":"a","x":10,"y":20}]})),
            Some(&layout_etag),
        )
        .await
        .unwrap();
    assert_eq!(changed_layout["layout"]["nodes"][0]["x"], 10);
    for (method, path, body) in [
        (Method::POST, "/api/runs/run_missing/resume", json!({})),
        (
            Method::POST,
            "/api/runs/run_missing/abort",
            json!({"reason":"stop"}),
        ),
        (
            Method::POST,
            "/api/runs/run_missing/land",
            json!({"force":false}),
        ),
        (
            Method::POST,
            "/api/runs/run_missing/gates/g/verdict",
            json!({"requestedSeq":1,"verdict":"pass"}),
        ),
        (
            Method::POST,
            "/api/runs/run_missing/gates/g/pick",
            json!({"requestedSeq":1,"attempt":1}),
        ),
    ] {
        let error = client
            .request(method, path, Some(body), None)
            .await
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("not_found"),
            "{path}: {error:#}"
        );
    }
    let invalid = client
        .request(
            Method::POST,
            "/api/runs",
            Some(json!({"expectedRev":"wrong"})),
            None,
        )
        .await
        .unwrap_err();
    assert!(format!("{invalid:#}").contains("bad_request"));
    for path in [
        "/api/runs/run_missing/resume",
        "/api/runs/run_missing/abort",
        "/api/runs/run_missing/land",
        "/api/runs/run_missing/gates/g/verdict",
        "/api/runs/run_missing/gates/g/pick",
    ] {
        let error = client
            .request(Method::POST, path, Some(json!("wrong type")), None)
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<NodeError>().unwrap().code,
            "bad_request",
            "{path}"
        );
    }
    let error = client
        .request(Method::GET, "/api/runs?mission=a&mission=b", None, None)
        .await
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<NodeError>().unwrap().code,
        "bad_request"
    );
    let missing_work = client
        .request(
            Method::POST,
            "/api/runs",
            Some(json!({"work":"missing-1"})),
            None,
        )
        .await
        .unwrap_err();
    assert!(format!("{missing_work:#}").contains("bad_request"));
    let (read, mission_etag) = client
        .request(Method::GET, "/api/missions/example", None, None)
        .await
        .unwrap();
    let rev = read["mission"]["rev"].as_i64().unwrap();
    let (deleted, _) = client
        .request(
            Method::DELETE,
            "/api/missions/example",
            Some(json!({"expectedRev":rev})),
            Some(&mission_etag),
        )
        .await
        .unwrap();
    assert_eq!(deleted["deletion"]["slug"], "example");
    let error = client
        .request(Method::GET, "/api/missions/missing", None, None)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("not_found"));
    let other = NodeClient::new(Some(&format!("{}/?node=elsewhere", node.url()))).unwrap();
    let error = other
        .request(Method::GET, "/api/missions", None, None)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("unknown_node"));
    node.stop().await;
}

fn lab(setup: &parvi_node_kit::Setup) -> anyhow::Result<parvi_node_kit::Built> {
    parvi_missions::build_with_executor(setup, Some(Box::new(parvi_mission_engine::LabExecutor)))
}
const LAB: Listing = Listing {
    build: lab,
    ..parvi_missions::LISTING
};

#[tokio::test(flavor = "multi_thread")]
async fn run_changes_arrive_on_the_socket_through_completion() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    let node = Node::start(&[sessions::LISTING, parvi_work::LISTING, LAB], "")
        .await
        .unwrap();
    let folder = node.dir().join("missions/missions");
    std::fs::create_dir_all(&folder).unwrap();
    let mission = include_str!("fixtures/solo-lab.mission.toml")
        .replace("to = \"end_done:in\"", "to = \"gate_review:in\"")
        + "\n[[gate]]\nid = \"gate_review\"\ntitle = \"Review\"\nkinds = [\"human\"]\n\n[[end]]\nid = \"end_rejected\"\ntitle = \"Rejected\"\noutcome = \"rejected\"\n\n[[connection]]\nid = \"edge_pass\"\nfrom = \"gate_review:pass\"\nto = \"end_done:in\"\n\n[[connection]]\nid = \"edge_fail\"\nfrom = \"gate_review:fail\"\nto = \"end_rejected:in\"\n";
    std::fs::write(folder.join("hello.mission.toml"), mission).unwrap();
    let client = NodeClient::new(Some(&format!("{}/", node.url()))).unwrap();
    std::fs::write(
        folder.join("admission-findings.mission.toml"),
        include_str!("fixtures/admission-findings.mission.toml"),
    )
    .unwrap();
    let rejected = client
        .request(
            Method::POST,
            "/api/runs",
            Some(json!({"mission":"admission-findings","inputCardId":"inp","expectedRev":1})),
            None,
        )
        .await
        .unwrap_err();
    let rejected = rejected.downcast_ref::<NodeError>().unwrap();
    assert_eq!(rejected.code, "run_admission_failed");
    let findings = rejected.findings.as_ref().unwrap().as_array().unwrap();
    assert!(findings.iter().any(|f| f["code"] == "missing_input"));
    assert!(
        findings
            .iter()
            .any(|f| f["code"] == "formation_without_slots")
    );
    let remote = include_str!("fixtures/solo-lab.mission.toml")
        .replace("slug = \"hello\"", "slug = \"remote\"")
        .replace("id = \"msn_hello\"", "id = \"msn_remote\"")
        .replace(
            "id = \"fmn_work\"",
            "id = \"fmn_work\"\ndevice = \"remote/linux\"",
        );
    std::fs::write(folder.join("remote.mission.toml"), remote).unwrap();
    let error = client.request(Method::POST, "/api/runs", Some(json!({"mission":"remote","inputCardId":"inp_hello","expectedRev":1,"inputs":{"brief":"hello"}})), None).await.unwrap_err();
    assert!(format!("{error:#}").contains("run_admission_failed"));
    let (health, _) = client
        .request(Method::GET, "/api/health", None, None)
        .await
        .unwrap();
    let place = format!(
        "{}/{}",
        health["device"].as_str().unwrap(),
        health["environment"].as_str().unwrap()
    );
    let local = include_str!("fixtures/solo-lab.mission.toml")
        .replace("slug = \"hello\"", "slug = \"local\"")
        .replace("id = \"msn_hello\"", "id = \"msn_local\"")
        .replace(
            "id = \"fmn_work\"",
            &format!("id = \"fmn_work\"\ndevice = \"{place}\""),
        );
    std::fs::write(folder.join("local.mission.toml"), local).unwrap();
    let (local_run, _) = client.request(Method::POST, "/api/runs", Some(json!({"mission":"local","inputCardId":"inp_hello","expectedRev":1,"inputs":{"brief":"hello"}})), None).await.unwrap();
    assert!(local_run["runId"].is_string());
    let (mut socket, _) = connect_async(node.ws()).await.unwrap();
    socket
        .send(Message::Binary(
            vec![parvi_missions_contract::WATCH_RUNS, 1].into(),
        ))
        .await
        .unwrap();
    loop {
        let Message::Binary(frame) = socket.next().await.unwrap().unwrap() else {
            continue;
        };
        if frame.first() == Some(&parvi_missions_contract::RUNS_SNAPSHOT) {
            break;
        }
    }
    let (started, _) = client.request(Method::POST, "/api/runs", Some(json!({"mission":"hello","inputCardId":"inp_hello","expectedRev":1,"inputs":{"brief":"hello"},"work":"gh:example/project#1"})), None).await.unwrap();
    let id = started["runId"].as_str().unwrap();
    let mut answered = false;
    let final_run = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        'stream: loop {
            let Message::Binary(frame) = socket.next().await.unwrap().unwrap() else {
                continue;
            };
            if frame.first() != Some(&parvi_missions_contract::RUNS_SNAPSHOT) {
                continue;
            }
            let runs: Vec<parvi_missions_contract::RunProjection> =
                serde_json::from_slice(&frame[1..]).unwrap();
            for run in runs.into_iter().filter(|r| r.run_id == id) {
                if let Some(gate) = run.waiting_gates.first() && !answered {
                    let path = format!("/api/runs/{id}/gates/{}/request", gate.gate_id);
                    let (request, _) = client.request(Method::GET, &path, None, None).await.unwrap();
                    assert_eq!(request["request"]["requestedSeq"], gate.requested_seq);
                    let path = format!("/api/runs/{id}/gates/{}/verdict", gate.gate_id);
                    for (body, status) in [
                        (json!({"requestedSeq":gate.requested_seq,"verdict":"pass","relayedBy":"-invalid"}), axum::http::StatusCode::BAD_REQUEST),
                        (json!({"requestedSeq":1,"verdict":"pass"}), axum::http::StatusCode::CONFLICT),
                    ] {
                        let error = client.request(Method::POST, &path, Some(body), None).await.unwrap_err();
                        assert_eq!(error.downcast_ref::<NodeError>().unwrap().status, status);
                    }
                    client.request(Method::POST, &path, Some(json!({"requestedSeq":gate.requested_seq,"verdict":"pass","reason":"  accepted\n","actor":"agent:http"})), None).await.unwrap();
                    let duplicate = client.request(Method::POST, &path, Some(json!({"requestedSeq":gate.requested_seq,"verdict":"pass"})), None).await.unwrap_err();
                    assert_eq!(duplicate.downcast_ref::<NodeError>().unwrap().status, axum::http::StatusCode::CONFLICT);
                    answered = true;
                }
                if run.r#final { break 'stream run; }
            }

        }
    })
    .await
    .expect("run reaches a final projection");
    assert!(answered);
    let ledger = parvi_mission_engine::Ledger::new(
        parvi_mission_engine::Store::root_in(node.dir()).join(format!("runs/{id}.ndjson")),
        id,
    );
    let events = ledger.read().unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.kind == "human_verdict_recorded"
                && event.data["reason"] == "  accepted\n"
                && event.actor == "agent:http"
                && event.data["decidedBy"] == "agent:http")
    );
    let unknown = client
        .request(
            Method::POST,
            &format!("/api/runs/{id}/resume"),
            Some(json!({"grant":true,"completedDispatchId":"dsp_1"})),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(
        unknown.downcast_ref::<NodeError>().unwrap().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(final_run.work, "gh:example/project#1");
    let (listed, _) = client
        .request(Method::GET, "/api/runs", None, None)
        .await
        .unwrap();
    let _: Vec<parvi_missions_contract::RunSummary> = serde_json::from_value(listed).unwrap();
    for suffix in [
        "",
        "/events",
        "/mission",
        "/seats",
        "/escalations",
        "/evidence/attempts",
    ] {
        client
            .request(Method::GET, &format!("/api/runs/{id}{suffix}"), None, None)
            .await
            .unwrap();
    }
    drop(socket);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn default_role_store_is_shared_and_isolated() {
    let areas = &[
        sessions::LISTING,
        parvi_work::LISTING,
        parvi_agents::LISTING,
        LAB,
    ];
    let node = Node::start(areas, "").await.unwrap();
    let other = Node::start(areas, "").await.unwrap();
    let root = parvi_mission_engine::Store::root_in(node.dir());
    let client = NodeClient::new(Some(&format!("{}/", node.url()))).unwrap();
    client
        .request(
            Method::POST,
            "/api/roles",
            Some(json!({"id":"fixture-writer","displayName":"Reviewer","kind":"specialist"})),
            None,
        )
        .await
        .unwrap();
    std::fs::create_dir_all(root.join("missions")).unwrap();
    let mission = include_str!("fixtures/solo-lab.mission.toml")
        .replace("agentId = \"builder\"", "agentId = \"fixture-writer\"");
    std::fs::write(root.join("missions/hello.mission.toml"), mission).unwrap();
    let (report, _) = client
        .request(Method::GET, "/api/missions/hello/validation", None, None)
        .await
        .unwrap();
    assert!(
        !report["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "unavailable_persona"),
        "{report}"
    );
    let (started, _) = client.request(Method::POST, "/api/runs", Some(json!({"mission":"hello","inputCardId":"inp_hello","expectedRev":1,"inputs":{"brief":"hello"}})), None).await.unwrap();
    assert!(started["runId"].is_string());
    let other_client = NodeClient::new(Some(&format!("{}/", other.url()))).unwrap();
    assert!(
        other_client
            .request(Method::GET, "/api/roles/fixture-writer", None, None)
            .await
            .is_err()
    );
    assert!(
        other_client
            .request(Method::GET, "/api/missions/hello", None, None)
            .await
            .is_err()
    );
    node.stop().await;
    other.stop().await;
}
