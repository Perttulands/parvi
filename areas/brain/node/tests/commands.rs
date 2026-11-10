use parvi_node_kit::cli;
use parvi_testkit::node::Node;
use serde_json::json;

#[tokio::test]
async fn brain_tools_and_cli_verbs_match_the_old_set() {
    assert_eq!(
        parvi_brain::LISTING.commands,
        &["brain_search", "brain_read"]
    );
    let node = Node::start(&[parvi_brain::LISTING], "").await.unwrap();
    let args = vec!["--node".to_string(), format!("{}/", node.url())];
    let mut out = Vec::new();
    cli::run("brain", &args, &mut out).await.unwrap();
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("parvi brain search"), "{output}");
    assert!(output.contains("parvi brain read"), "{output}");
    let missing = node
        .call("brain_search", json!({"query":"north"}))
        .await
        .unwrap_err();
    assert!(missing.message.contains("[brain].citadel"));
    node.stop().await;
}
