#![cfg(unix)]
use axum::http::Method;
use parvi_node_kit::{NodeClient, NodeError};
use parvi_testkit::{fresh_dir, node::Node, tmux::write_script};
use std::time::{Duration, Instant};

#[tokio::test]
async fn configured_help_and_timeout() {
    let dir = fresh_dir("flags-route");
    let fast = dir.join("fast");
    let slow = dir.join("slow");
    let calls = dir.join("calls");
    write_script(
        &fast,
        format!(
            "#!/bin/sh\n[ \"$1\" = --help ] || exit 1\necho x >> '{}'\nprintf 'Options:\\n  -v, --verbose  Verbose output\\n'\n",
            calls.display()
        ),
    );
    write_script(&slow, "#!/bin/sh\nexec sleep 10\n");
    let launch = dir.join("launch.conf");
    std::fs::write(
        &launch,
        format!(
            "harness fast {}\nharness slow {}\n",
            fast.display(),
            slow.display()
        ),
    )
    .unwrap();
    let node = Node::start(
        &[parvi_terminals::LISTING],
        &format!("[terminals]\nlaunch = {:?}\n", launch),
    )
    .await
    .unwrap();
    let client = NodeClient::new(Some(&node.url())).unwrap();
    let (answer, _) = client
        .request(Method::GET, "/api/launch/flags?harness=fast", None, None)
        .await
        .unwrap();
    assert_eq!(answer[0]["name"], "--verbose");
    assert_eq!(answer[0]["description"], "Verbose output");
    client
        .request(Method::GET, "/api/launch/flags?harness=fast", None, None)
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(), 1);
    std::fs::File::open(&fast)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(100)),
        )
        .unwrap();
    client
        .request(Method::GET, "/api/launch/flags?harness=fast", None, None)
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(), 2);

    let error = client
        .request(Method::GET, "/api/launch/flags?harness=unknown", None, None)
        .await
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<NodeError>().unwrap().code,
        "unknown_harness"
    );
    let started = Instant::now();
    let error = client
        .request(Method::GET, "/api/launch/flags?harness=slow", None, None)
        .await
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<NodeError>().unwrap().code,
        "harness_help_timeout"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    node.stop().await;
    std::fs::remove_dir_all(dir).unwrap();
}
