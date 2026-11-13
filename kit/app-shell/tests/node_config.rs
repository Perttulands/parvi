//! The Settings page shows the connected node's `config.toml` as the node
//! loaded it, read-only, against a testkit node holding one area defined
//! here.

use gpui::{TestAppContext, VisualTestContext};
use parvi_node_kit::{Area, Built, Listing, Setup};
use parvi_testkit::node::Node;
use std::time::{Duration, Instant};

/// An area that reads its section and does nothing else.
fn files(_: &Setup) -> anyhow::Result<Built> {
    struct Files;
    impl Area for Files {}
    Built::now(Files)
}

const FILES: Listing = Listing {
    name: "files",
    kinds: &[],
    commands: &[],
    build: files,
};

/// Whether the element named `selector` is drawn.
fn drawn(cx: &mut VisualTestContext, selector: String) -> bool {
    cx.debug_bounds(Box::leak(selector.into_boxed_str()))
        .is_some()
}

#[gpui::test]
fn the_settings_page_shows_the_nodes_config_file(cx: &mut TestAppContext) {
    let rt = tokio::runtime::Runtime::new().expect("a runtime");
    let long = format!("/srv/{}", "very-long-project-name/".repeat(30));
    let config = format!("[files]\nroots = [\"/srv/notes\", {long:?}]\n");
    let node = rt
        .block_on(Node::start(&[FILES], &config))
        .expect("the node starts");
    let file = node.dir().join("config.toml");
    let settings = parvi_testkit::fresh_dir("parvi-app-node-config").join("app.json");
    // The node and the link run on their own threads.
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (url, path) = (node.url(), settings.clone());
    let field = format!("setting-app-node:{url}");
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[], Some(url), Some(path), window, cx)
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("settings"));

    // The app's own settings show first; the node's are a group of their
    // own, each key a named line and each root a line of its own.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !drawn(cx, field.clone()) {
        assert!(Instant::now() < deadline, "the node field is not shown");
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
    let field_bounds = cx.debug_bounds(Box::leak(field.into_boxed_str())).unwrap();
    let group = cx
        .debug_bounds("settings-group-node")
        .expect("the node's group");
    cx.simulate_click(group.center(), gpui::Modifiers::none());
    let path = format!("node-config:path:{}", file.display());
    let short = "node-config:files:roots:/srv/notes".to_string();
    let roots = format!("node-config:files:roots:{long}");
    loop {
        cx.run_until_parked();
        if drawn(cx, path.clone()) && drawn(cx, roots.clone()) && drawn(cx, short.clone()) {
            break;
        }
        assert!(Instant::now() < deadline, "the node's config is not shown");
        std::thread::sleep(Duration::from_millis(20));
    }
    let roots_bounds = cx.debug_bounds(Box::leak(roots.into_boxed_str())).unwrap();
    assert!(
        field_bounds.size.width >= gpui::px(300.),
        "node URL has room"
    );
    assert!(
        roots_bounds.size.width <= field_bounds.size.width + gpui::px(1.),
        "config stays in the value column"
    );
    assert!(
        roots_bounds.size.height > gpui::px(30.),
        "a long config value wraps"
    );
    rt.block_on(node.stop());
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}
