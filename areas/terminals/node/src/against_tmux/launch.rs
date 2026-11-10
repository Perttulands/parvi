//! The launcher and the hub against a real tmux on scratch sockets.

use std::path::Path;

use parvi_term::engine::Engine;
use parvi_testkit::tmux::*;

use crate::agent_events::hooks::Hooks;
use crate::launch::{Config, Launch, command_line};

#[tokio::test(flavor = "multi_thread")]
async fn launches_in_the_folder_at_the_size_and_deletes_exactly_one() {
    let Some(server) = Server::start() else {
        return;
    };
    server.new_session("keep", 80, 24);
    let folder = std::env::temp_dir().join(format!("{}-launch", server.name));
    std::fs::create_dir_all(&folder).unwrap();
    let config = Config::parse("harness toucher touch\nsize 120x40").unwrap();
    let launch = Launch {
        harness: "toucher".into(),
        folder: folder.to_string_lossy().into_owned(),
        name: "launched".into(),
        flags: "launched-here".into(),
        user: String::new(),
        notify: false,
        size: None,
    };
    let engine = parvi_term::engine::Engine::start(server.socket());
    let first = crate::launch::launch(&engine, &config, &launch, NO_HOOKS)
        .await
        .unwrap();
    assert!(first.id.starts_with('$'), "{first:?}");
    assert_eq!(first.warning, None, "notify was not asked for");
    let id = first.id;

    // The previewed command ran, in the folder.
    assert_eq!(command_line("touch", &launch.flags), "touch launched-here");
    eventually("the harness to run in the folder", || {
        folder.join("launched-here").exists().then_some(())
    })
    .await;
    // Sized once to the configured size, and tmux still sizes it by its
    // latest client, not by a pinned size.
    assert_eq!(server.window_size("launched"), "120x40");
    assert_eq!(
        server.display("launched", "#{window-size}").trim(),
        "latest"
    );
    assert_eq!(
        server.display("launched", "#{session_attached}").trim(),
        "1"
    );

    // Hooks that can't be installed don't stop a launch: it runs without
    // them and says why, and no session is left behind by a failure.
    let hooks = Hooks::new(
        Path::new("/opt/parvi"),
        "127.0.0.1:1".parse().unwrap(),
        "/dev/null/agent-hooks".into(),
        "/dev/null/config.toml".into(),
    )
    .unwrap();
    let claude = Config::parse(&format!(
        "harness claude {}/claude\nsize 120x40",
        folder.display()
    ))
    .unwrap();
    let hooked = Launch {
        harness: "claude".into(),
        name: "unhooked".into(),
        flags: String::new(),
        notify: true,
        ..launch.clone()
    };
    let launched = crate::launch::launch(&engine, &claude, &hooked, Ok(&hooks))
        .await
        .unwrap();
    let warning = launched.warning.unwrap();
    assert!(
        warning.starts_with("no completion notices: creating /dev/null/agent-hooks"),
        "{warning}"
    );
    server.run(&["kill-session", "-t", "unhooked"]);
    let off = Launch {
        name: "off".into(),
        ..hooked.clone()
    };
    let launched = crate::launch::launch(&engine, &claude, &off, Err("no hooks here"))
        .await
        .unwrap();
    assert_eq!(
        launched.warning.as_deref(),
        Some("no completion notices: no hooks here")
    );
    server.run(&["kill-session", "-t", "off"]);

    // Launched into a window, it is given that window's grid instead of the
    // launcher's (#161), and is still sized by its viewers after.
    let fitted = Launch {
        name: "fitted".into(),
        size: Some((97, 31)),
        ..launch.clone()
    };
    crate::launch::launch(&engine, &config, &fitted, NO_HOOKS)
        .await
        .unwrap();
    assert_eq!(server.window_size("fitted"), "97x31");
    assert_eq!(server.display("fitted", "#{window-size}").trim(), "latest");
    let viewer = server.viewer("fitted", 110, 35);
    window_becomes(&server, "fitted", "110x35").await;
    viewer.detach();
    server.run(&["kill-session", "-t", "fitted"]);

    // A taken name is refused with tmux's reason.
    let again = crate::launch::launch(&engine, &config, &launch, NO_HOOKS).await;
    assert!(format!("{:#}", again.unwrap_err()).contains("duplicate"));

    // For another user, the server must run as them, and ~ is the home that
    // server knows. Here the other user is whoever runs the test.
    server.run(&["set-environment", "-g", "HOME", &folder.to_string_lossy()]);
    let mut theirs = Launch {
        harness: "shell".into(),
        folder: "~".into(),
        name: "theirs".into(),
        flags: String::new(),
        user: "someone-else".into(),
        notify: false,
        size: None,
    };
    let refused = crate::launch::launch(&engine, &config, &theirs, NO_HOOKS).await;
    assert!(format!("{:#}", refused.unwrap_err()).contains("runs as"));
    theirs.user = whoami();
    crate::launch::launch(&engine, &config, &theirs, NO_HOOKS)
        .await
        .unwrap();
    assert_eq!(
        server.display("theirs", "#{pane_current_path}").trim(),
        folder.to_string_lossy()
    );
    server.run(&["kill-session", "-t", "theirs"]);

    crate::launch::delete(&server.socket(), &id).await.unwrap();
    let left = server.run(&["list-sessions", "-F", "#{session_name}"]);
    assert_eq!(left.trim(), "keep");
    assert!(
        crate::launch::delete(&server.socket(), "keep")
            .await
            .is_err()
    );
    let _ = std::fs::remove_dir_all(&folder);
}

/// A launch that does not ask for hooks gets none either way.
const NO_HOOKS: Result<&Hooks, &str> = Err("no hooks in this test");

#[tokio::test(flavor = "multi_thread")]
async fn window_picker_lists_the_active_pane_of_each_window_on_its_server() {
    use crate::hub::{Hub, Server as HubServer};
    let Some(first) = Server::start() else { return };
    let Some(second) = Server::start() else {
        return;
    };
    first.new_session("first", 80, 24);
    second.new_session("alpha", 80, 24);
    second.run(&["rename-window", "-t", "alpha:0", "editor"]);
    second.run(&["new-window", "-d", "-t", "alpha:3", "-n", "build"]);
    second.run(&["split-window", "-t", "alpha:3"]);
    let active = second.run(&["display-message", "-p", "-t", "alpha:3", "#{pane_id}"]);
    let active: u32 = active.trim().trim_start_matches('%').parse().unwrap();
    let a = std::sync::Arc::new(Engine::start(first.socket()));
    let b = std::sync::Arc::new(Engine::start(second.socket()));
    let hub = Hub::new(vec![
        HubServer {
            user: None,
            engine: a.clone(),
        },
        HubServer {
            user: Some("other".into()),
            engine: b.clone(),
        },
    ])
    .unwrap();
    let id = eventually("session listed", || {
        hub.sessions()
            .into_iter()
            .find(|s| s.name == "alpha")
            .map(|s| s.id)
    })
    .await;
    let windows = hub.windows(&id).await.unwrap();
    assert_eq!(
        windows
            .iter()
            .map(|w| (w.index, w.name.as_str()))
            .collect::<Vec<_>>(),
        [(0, "editor"), (3, "build")]
    );
    assert_eq!(windows[1].pane, (1 << 24) | active);
    assert!(hub.windows("$999999").await.is_err());
    a.stop().await;
    b.stop().await;
}
