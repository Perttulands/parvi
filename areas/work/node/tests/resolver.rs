//! Work provides reference resolution to the node's other areas.
use parvi_core::Error;
use parvi_node_kit::{Built, Listing, Setup};
use parvi_testkit::node::Node;
use parvi_work::{Bd, Gh};
use parvi_work_contract::Resolver;
use std::path::PathBuf;
use std::sync::OnceLock;

/// A folder holding one Beads store with the prefix `tst`, and a bd that
/// answers its prefix.
fn fixture() -> &'static (PathBuf, String) {
    static FIXTURE: OnceLock<(PathBuf, String)> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let dir = parvi_testkit::fresh_dir("parvi-work-resolver");
        let store = dir.join("store");
        std::fs::create_dir_all(store.join(".beads")).unwrap();
        let bd = dir.join("bd");
        // Written by a child, so this process never holds the script open
        // for writing while another thread forks (ETXTBSY).
        let wrote = std::process::Command::new("sh")
            .args(["-c", "printf '%s\\n' '#!/bin/sh' 'echo {\\\"value\\\":\\\"tst\\\"}' > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(&bd)
            .status()
            .unwrap();
        assert!(wrote.success());
        (store, bd.display().to_string())
    })
}

fn build(setup: &Setup) -> anyhow::Result<Built> {
    parvi_work::build_with(setup, Bd::new(fixture().1.clone()), Gh::default())
}

const WORK: Listing = Listing {
    build,
    ..parvi_work::LISTING
};

#[tokio::test(flavor = "multi_thread")]
async fn work_resolves_references_for_other_areas() {
    let store = fixture().0.display().to_string();
    let config = format!("[work]\nprojects = [\"beads {store}\"]\n");
    let node = Node::start(&[WORK], &config).await.unwrap();
    let resolver = node.provided::<dyn Resolver>().unwrap();
    let reference = resolver.resolve("tst-1").await.unwrap();
    assert_eq!(
        reference.to_string(),
        format!("bead:{}:{store}#tst-1", node.id())
    );
    let unknown: Error = resolver.resolve("zz-1").await.unwrap_err();
    assert_eq!(unknown.code, parvi_work_contract::BAD_REQUEST);
    assert!(
        unknown.message.contains("no Beads store"),
        "{}",
        unknown.message
    );
    node.stop().await;

    let off = Node::start(&[WORK], "[work]\nenabled = false\n")
        .await
        .unwrap();
    let Err(missing) = off.provided::<dyn Resolver>() else {
        panic!("Work is off, yet a resolver is provided");
    };
    assert_eq!(missing.code, "not_provided");
    off.stop().await;
    std::fs::remove_dir_all(fixture().0.parent().unwrap()).unwrap();
}
