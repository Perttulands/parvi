//! The scheduled area over the testkit's stand-in sessions: no tmux and no
//! terminals, so these run everywhere. It delivers a run into the right
//! session, follows a rename, says why a session refused, and is refused
//! when it is listed before the area providing sessions.

use parvi_node_kit::Build;
use parvi_testkit::node::Node;
use parvi_testkit::sessions;
use serde_json::json;

const AREAS: &[(&str, Build)] = &[
    (sessions::NAME, sessions::build),
    (parvi_scheduled::NAME, parvi_scheduled::build),
];

#[tokio::test(flavor = "multi_thread")]
async fn a_run_is_delivered_to_the_session_a_task_names() {
    let node = Node::start(AREAS, "").await.unwrap();
    let fake = node.fake_sessions();
    fake.add("alpha");
    fake.add_of("bob", "alpha");
    let made = node
        .call(
            "scheduled_create",
            json!({ "name": "nudge", "prompt": "wake up", "sessions": ["bob/alpha"], "every": 60 }),
        )
        .await
        .unwrap();
    let id = made["task"]["id"].as_str().unwrap();
    node.call("scheduled_run", json!({ "id": id }))
        .await
        .unwrap();
    assert_eq!(
        fake.delivered(),
        [sessions::Delivered {
            session: "alpha".into(),
            text: "wake up".into(),
            enter: true
        }]
    );
    assert_eq!(fake.delivered().len(), 1, "only bob's alpha got it");

    let unknown = node
        .call(
            "scheduled_create",
            json!({ "name": "x", "prompt": "y", "sessions": ["nobody/alpha"], "every": 5 }),
        )
        .await;
    assert!(unknown.unwrap_err().message.contains("nobody"));
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_renamed_session_keeps_its_tasks_and_a_refusal_is_reported() {
    let node = Node::start(AREAS, "").await.unwrap();
    let fake = node.fake_sessions();
    fake.add("alpha");
    let made = node
        .call(
            "scheduled_create",
            json!({ "name": "nudge", "prompt": "hi", "sessions": ["alpha"], "every": 60 }),
        )
        .await
        .unwrap();
    let id = made["task"]["id"].as_str().unwrap().to_owned();

    fake.rename("alpha", "beta");
    let mut shown = json!(null);
    for _ in 0..100 {
        shown = node.call("scheduled_list", json!({})).await.unwrap();
        if shown["tasks"][0]["sessions"] == json!(["beta"]) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(shown["tasks"][0]["sessions"], json!(["beta"]), "{shown}");

    fake.refuse("beta");
    node.call("scheduled_run", json!({ "id": id }))
        .await
        .unwrap();
    assert!(fake.delivered().is_empty());
    let task = node
        .call("scheduled_show", json!({ "id": id }))
        .await
        .unwrap();
    assert!(
        task["task"]["recentRuns"].to_string().contains("refused"),
        "{task}"
    );
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn scheduled_listed_before_the_sessions_area_is_refused_with_what_to_change() {
    let early: &[(&str, Build)] = &[
        (parvi_scheduled::NAME, parvi_scheduled::build),
        (sessions::NAME, sessions::build),
    ];
    let error = match Node::start(early, "").await {
        Ok(_) => panic!("scheduled started without sessions"),
        Err(error) => format!("{error:#}"),
    };
    assert!(error.contains("starting scheduled"), "{error}");
    assert!(error.contains("must be built before it"), "{error}");
}
