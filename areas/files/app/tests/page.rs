//! The Files page draws a node's real listing.
use gpui::{Modifiers, ParentElement, TestAppContext, VisualTestContext};
use parvi_testkit::node::Node;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

thread_local! {
    static SENT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    static SEARCH_PAGE: std::cell::RefCell<Option<gpui::Entity<parvi_files_app::FilesPage>>> = const { std::cell::RefCell::new(None) };
    static TERMINAL_PAGE: std::cell::RefCell<Option<gpui::Entity<parvi_terminals_app::Terminals>>> = const { std::cell::RefCell::new(None) };
}

fn observed_terminals(
    host: parvi_app_kit::Host,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> parvi_app_kit::Mounted {
    parvi_app_kit::mount(host, window, cx, |host, window, cx| {
        TERMINAL_PAGE.with(|page| *page.borrow_mut() = Some(cx.entity()));
        parvi_terminals_app::Terminals::new(host, window, cx)
    })
}

fn observed_files(
    host: parvi_app_kit::Host,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> parvi_app_kit::Mounted {
    parvi_app_kit::mount(host, window, cx, |host, window, cx| {
        SEARCH_PAGE.with(|page| *page.borrow_mut() = Some(cx.entity()));
        parvi_files_app::FilesPage::new(host, window, cx)
    })
}

struct Toy;
impl parvi_app_kit::Page for Toy {
    const ID: &'static str = "toy";
    const TITLE: &'static str = "Toy";
    const ANSWERS: &'static [parvi_app_kit::Answers] = &[parvi_app_kit::Answers::Send];
    fn answer(
        &mut self,
        ask: parvi_app_kit::Ask,
        _: &'static str,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) {
        if let parvi_app_kit::Ask::Send { reference, .. } = ask {
            SENT.with(|sent| *sent.borrow_mut() = Some(reference));
        }
    }
}
impl gpui::Render for Toy {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div().child("Toy page")
    }
}
fn toy(
    host: parvi_app_kit::Host,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> parvi_app_kit::Mounted {
    parvi_app_kit::mount(host, window, cx, |_, _, _| Toy)
}

#[gpui::test]
fn files_dock_browses_sends_and_remembers_beside_a_page(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-dock");
    std::fs::create_dir(root.join("folder")).unwrap();
    let file = root.join("folder/note.txt");
    std::fs::write(&file, "hello").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let settings = root.join("settings.json");
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(
            &[toy, parvi_files_app::build],
            Some(url),
            Some(settings),
            w,
            cx,
        )
    });
    until(cx, "node connected", |cx| {
        shell.read_with(cx, |s, cx| {
            !parvi_app_kit::Host::new(s.hub()).nodes(cx).is_empty()
        })
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    assert!(shell.update(cx, |s, cx| {
        parvi_app_kit::chords::rows(&s.chords(cx))
            .iter()
            .any(|row| row.label == "Files panel")
    }));
    cx.simulate_keystrokes("alt-o");
    until(cx, "dock folder", |cx| {
        cx.debug_bounds("file-row-folder").is_some()
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    click(cx, "file-row-folder");
    until(cx, "dock file", |cx| {
        cx.debug_bounds("file-row-note.txt").is_some()
    });
    let modified = std::fs::metadata(&file)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let date = parvi_files_app::store::format::modified(modified);
    let name = cx.debug_bounds("file-name-note.txt").expect("name drawn");
    let size = cx
        .debug_bounds("file-size-note.txt-5 B")
        .expect("size drawn");
    let date_selector: &'static str =
        Box::leak(format!("file-modified-note.txt-{date}").into_boxed_str());
    let modified = cx.debug_bounds(date_selector).expect("modified time drawn");
    assert!(name.right() <= size.left() && size.right() <= modified.left());
    click(cx, "file-row-note.txt");
    until(cx, "dock viewer", |cx| {
        cx.debug_bounds("files-send").is_some()
    });
    SENT.with(|sent| *sent.borrow_mut() = None);
    click(cx, "files-send");
    until(cx, "sent reference", |_| {
        SENT.with(|sent| sent.borrow().is_some())
    });
    assert_eq!(
        SENT.with(|sent| sent.borrow().clone()),
        Some(format!("path {}", file.display()))
    );
    cx.simulate_keystrokes("alt-o");
    assert!(cx.debug_bounds("dock-files").is_none());
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    cx.simulate_keystrokes("alt-o");
    until(cx, "remembered file", |cx| {
        cx.debug_bounds("file-view-note.txt").is_some() && cx.debug_bounds("files-send").is_some()
    });
    click(cx, "files-back");
    until(cx, "back to the folder", |cx| {
        cx.debug_bounds("file-row-note.txt").is_some() && cx.debug_bounds("files-send").is_none()
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

struct Running {
    url: String,
    node: String,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}
fn start(root: std::path::PathBuf) -> Running {
    start_with_terminal(root, None)
}
fn start_with_terminal(root: std::path::PathBuf, socket: Option<std::path::PathBuf>) -> Running {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let mut config = format!(
                "[files]\nroots = [{}]\n",
                serde_json::to_string(root.to_str().unwrap()).unwrap()
            );
            let areas = if let Some(socket) = socket {
                config.push_str(&format!("[terminals]\nsocket = {:?}\n", socket));
                vec![parvi_files::LISTING, parvi_terminals::LISTING]
            } else {
                vec![parvi_files::LISTING]
            };
            let node = Node::start(&areas, &config).await.unwrap();
            ready.send((node.url(), node.id())).unwrap();
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            node.stop().await;
        });
    });
    let (url, node) = wait.recv().unwrap();
    Running {
        url,
        node,
        stop,
        thread: Some(thread),
    }
}

/// Browse, find, edit and send the saved file to a real scratch session.
#[cfg(unix)]
#[gpui::test]
fn terminal_dock_search_and_printed_path_open_beside(cx: &mut TestAppContext) {
    let Some(tmux) = parvi_testkit::tmux::Server::start_in("files-finish-terminal") else {
        assert!(std::env::var_os("PARVI_REQUIRE_TMUX").is_none());
        return;
    };
    let root = parvi_testkit::fresh_dir("files-finish-terminal-root");
    std::fs::create_dir(root.join("sub")).unwrap();
    let file = root.join("sub/note.txt");
    std::fs::write(
        &file,
        format!(
            "{}find marker\n{}",
            "line\n".repeat(11),
            "tail\n".repeat(30)
        ),
    )
    .unwrap();
    let sub = root.join("sub").to_string_lossy().into_owned();
    tmux.run(&[
        "new-session",
        "-d",
        "-s",
        "alpha",
        "-x",
        "100",
        "-y",
        "30",
        "-c",
        &sub,
        "env PS1='$ ' sh",
    ]);
    let running = start_with_terminal(root.clone(), Some(tmux.socket_path()));
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(
            &[observed_terminals, parvi_files_app::build],
            Some(url),
            None,
            w,
            cx,
        )
    });
    let row: &'static str = Box::leak(format!("row-{}::alpha", running.node).into_boxed_str());
    until(cx, "terminal session", |cx| cx.debug_bounds(row).is_some());
    click(cx, row);
    until(cx, "attach", |cx| cx.debug_bounds("peek-attach").is_some());
    click(cx, "peek-attach");
    let terminal = TERMINAL_PAGE.with(|p| p.borrow().as_ref().unwrap().clone());
    until(cx, "terminal painted", |cx| {
        terminal.read_with(cx, |p, _| p.geo.borrow()[0].origin.is_some())
    });
    // The dock opens at the session's folder.
    cx.simulate_keystrokes("alt-o");
    until(cx, "dock at the session's folder", |cx| {
        cx.debug_bounds("file-row-note.txt").is_some()
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.shown_page()),
        Some("terminals")
    );
    click(cx, "files-up");
    until(cx, "back at root", |cx| {
        cx.debug_bounds("file-row-sub").is_some()
    });
    click(cx, "files-search");
    cx.simulate_input("note");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "name hit", |cx| {
        cx.debug_bounds("find-label-sub/note.txt").is_some()
    });
    click(cx, "files-search");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("find marker");
    click(cx, "search-mode");
    until(cx, "content hit", |cx| {
        cx.debug_bounds("find-label-sub/note.txt:12").is_some()
    });
    click(cx, "find-result-note.txt");
    until(cx, "dock file", |cx| {
        cx.debug_bounds("file-view-note.txt").is_some()
            && cx.debug_bounds("files-code-top-9").is_some()
    });
    cx.simulate_keystrokes("alt-o");
    until(cx, "dock closed", |cx| {
        cx.debug_bounds("dock-files").is_none()
    });
    let path = file.to_string_lossy().into_owned();
    tmux.run(&[
        "send-keys",
        "-t",
        "alpha",
        &format!("printf '%s\\n' '{}'", path),
        "Enter",
    ]);
    until(cx, "printed path", |cx| {
        terminal.read_with(cx, |p, _| {
            p.window_text(0)
                .is_some_and(|text| text.lines().any(|line| line.trim() == path))
        })
    });
    let position = terminal.read_with(cx, |p, _| {
        let text = p.window_text(0).unwrap();
        let (row, line) = text
            .lines()
            .enumerate()
            .find(|(_, line)| line.trim() == path)
            .unwrap();
        let col = line.find(&path).unwrap();
        let geo = p.geo.borrow()[0];
        let origin = geo.origin.unwrap();
        let cell = geo.cell.unwrap();
        gpui::point(
            origin.x + cell.width * (col as f32 + 1.5),
            origin.y + cell.height * (row as f32 + 0.5),
        )
    });
    cx.simulate_click(position, Modifiers::none());
    until(cx, "path viewer beside terminal", |cx| {
        cx.debug_bounds("terminals-beside").is_some()
            && cx.debug_bounds("file-view-note.txt").is_some()
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.shown_page()),
        Some("terminals")
    );
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[gpui::test]
fn browse_find_edit_save_and_send_to_a_session(cx: &mut TestAppContext) {
    let Some(tmux) = parvi_testkit::tmux::Server::start_in("files-journey") else {
        assert!(std::env::var_os("PARVI_REQUIRE_TMUX").is_none());
        return;
    };
    let root = parvi_testkit::fresh_dir("files-journey-root");
    std::fs::create_dir(root.join("folder")).unwrap();
    let file = root.join("folder/needle.txt");
    std::fs::write(&file, "before").unwrap();
    let received = tmux.dir().join("received");
    tmux.new_session("alpha", 100, 30);
    tmux.run(&[
        "send-keys",
        "-t",
        "alpha",
        &format!("cat > '{}'", received.display()),
        "Enter",
    ]);
    let running = start_with_terminal(root.clone(), Some(tmux.socket_path()));
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_terminals_app::build, parvi_files_app::build],
            Some(url),
            None,
            window,
            cx,
        )
    });
    let row: &'static str = Box::leak(format!("row-{}::alpha", running.node).into_boxed_str());
    until(cx, "session in rail", |cx| cx.debug_bounds(row).is_some());
    click(cx, row);
    until(cx, "attach session", |cx| {
        cx.debug_bounds("peek-attach").is_some()
    });
    click(cx, "peek-attach");
    click(cx, "tab-files");
    browse_root(cx);
    until(cx, "folder", |cx| {
        cx.debug_bounds("file-row-folder").is_some()
    });
    click(cx, "file-row-folder");
    until(cx, "folder listing", |cx| {
        cx.debug_bounds("file-row-needle.txt").is_some()
    });
    cx.simulate_keystrokes("/");
    cx.simulate_input("needle");
    cx.simulate_keystrokes("enter");
    until(cx, "search result", |cx| {
        cx.debug_bounds("find-result-needle.txt").is_some()
    });
    click(cx, "find-result-needle.txt");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    click(cx, "files-edit");
    until(cx, "editor", |cx| cx.debug_bounds("files-save").is_some());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("after");
    cx.simulate_keystrokes("ctrl-s");
    until(cx, "saved file", |_| {
        std::fs::read(&file).ok().as_deref() == Some(b"after")
    });
    click(cx, "files-send");
    until(cx, "send drawer", |cx| cx.debug_bounds("send-go").is_some());
    assert_eq!(
        shell.read_with(cx, |s, _| s.shown_page()),
        Some("terminals")
    );
    click(cx, "send-go");
    let expected = format!("path {}", file.display());
    until(cx, "reference delivered to session", |_| {
        std::fs::read_to_string(&received).is_ok_and(|text| text.contains(&expected))
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut ok: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        if ok(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn click(cx: &mut VisualTestContext, id: &'static str) {
    let point = cx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("missing {id}"))
        .center();
    cx.simulate_click(point, Modifiers::none());
}

fn browse_root(cx: &mut VisualTestContext) {
    until(cx, "root browser", |cx| {
        cx.debug_bounds("files-browse-root-0").is_some()
            || cx.debug_bounds("files-search").is_some()
    });
    if cx.debug_bounds("files-browse-root-0").is_some() {
        click(cx, "files-browse-root-0");
    }
}

#[gpui::test]
fn projects_landing_groups_repositories_sessions_and_scopes_find(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-projects-landing");
    for dir in [
        "one", "two", "plain", "three", "four", "five", "six", "seven", "eight",
    ] {
        std::fs::create_dir(root.join(dir)).unwrap();
    }
    for dir in ["three", "four", "five", "six", "seven", "eight"] {
        std::fs::create_dir(root.join(dir).join(".git")).unwrap();
    }
    std::fs::create_dir(root.join("one/.git")).unwrap();
    std::fs::write(root.join("two/.git"), "gitdir: elsewhere").unwrap();
    std::fs::write(root.join("one/README.md"), "readme").unwrap();
    std::fs::write(root.join("one/AGENTS.md"), "agents").unwrap();
    std::fs::write(root.join("one/VISION.md"), "vision").unwrap();
    std::fs::write(root.join("one/needle-inside.txt"), "inside").unwrap();
    std::fs::write(root.join("two/needle-outside.txt"), "outside").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    let host = shell.read_with(cx, |s, _| {
        parvi_app_kit::Host::new(s.hub()).for_page("test-sessions", &[])
    });
    let session = parvi_terminals_contract::Session {
        node: running.node.clone(),
        name: "working".into(),
        cwd: Some(root.join("one/src").to_string_lossy().into_owned()),
        ..Default::default()
    };
    cx.update(|_, cx| {
        host.publish(
            parvi_terminals_contract::SessionList {
                sessions: vec![session],
                ..Default::default()
            },
            cx,
        )
        .unwrap()
    });
    until(cx, "project cards", |cx| {
        cx.debug_bounds("files-project-one").is_some()
            && cx.debug_bounds("files-project-two").is_some()
            && cx
                .debug_bounds("files-project-session-one-working")
                .is_some()
    });
    assert!(cx.debug_bounds("files-project-plain").is_none());
    let list = cx.debug_bounds("files-projects-list").unwrap();
    for id in [
        "files-browse-root-0",
        "files-project-one",
        "files-project-two",
        "files-project-three",
        "files-project-four",
        "files-project-five",
        "files-project-six",
        "files-project-seven",
        "files-project-eight",
    ] {
        let item = cx.debug_bounds(id).expect("project or browse drawn");
        assert!(
            item.top() >= list.top() && item.bottom() <= list.bottom(),
            "{id} is visible without scrolling"
        );
    }
    for doc in [
        "files-project-doc-one-README.md",
        "files-project-doc-one-AGENTS.md",
        "files-project-doc-one-VISION.md",
    ] {
        assert!(cx.debug_bounds(doc).is_some());
    }
    click(cx, "files-project-doc-one-README.md");
    until(cx, "document opened", |cx| {
        cx.debug_bounds("file-view-README.md").is_some()
    });
    click(cx, "files-projects");
    until(cx, "projects again", |cx| {
        cx.debug_bounds("files-project-one").is_some()
    });
    click(cx, "files-project-one");
    until(cx, "project browser", |cx| {
        cx.debug_bounds("file-row-needle-inside.txt").is_some()
    });
    cx.simulate_keystrokes("/");
    cx.simulate_input("needle");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "scoped find", |cx| {
        cx.debug_bounds("find-result-needle-inside.txt").is_some()
    });
    assert!(cx.debug_bounds("find-result-needle-outside.txt").is_none());
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

fn menu_on(cx: &mut VisualTestContext, row: &'static str) {
    let at = cx.debug_bounds(row).expect("file row").center();
    cx.simulate_mouse_down(at, gpui::MouseButton::Right, Modifiers::none());
    until(cx, "file menu", |cx| {
        cx.debug_bounds("file-menu-open").is_some()
    });
}

fn drop_files(cx: &mut VisualTestContext, row: &'static str, files: Vec<std::path::PathBuf>) {
    let position = cx.debug_bounds(row).expect("drop target").center();
    cx.simulate_event(gpui::FileDropEvent::Entered {
        position,
        paths: gpui::ExternalPaths(files.into_iter().collect()),
    });
    cx.simulate_event(gpui::FileDropEvent::Submit { position });
}

#[cfg(unix)]
#[gpui::test]
fn upload_drop_conflicts_picker_and_node_error(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let root = parvi_testkit::fresh_dir("files-upload-root");
    let sources = parvi_testkit::fresh_dir("files-upload-sources");
    std::fs::create_dir(root.join("target")).unwrap();
    std::fs::create_dir(root.join("locked")).unwrap();
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let first = sources.join("one.bin");
    let second = sources.join("two.txt");
    std::fs::write(&first, [0, 255, 128, 42]).unwrap();
    std::fs::write(&second, b"second file").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    browse_root(cx);
    until(cx, "folder rows", |cx| {
        cx.debug_bounds("file-row-target").is_some() && cx.debug_bounds("file-row-locked").is_some()
    });
    drop_files(cx, "file-row-target", vec![first.clone(), second.clone()]);
    until(cx, "two files uploaded", |_| {
        std::fs::read(root.join("target/one.bin")).ok().as_deref() == Some(&[0, 255, 128, 42])
            && std::fs::read(root.join("target/two.txt")).ok().as_deref() == Some(b"second file")
    });
    std::fs::write(&second, b"changed").unwrap();
    drop_files(cx, "file-row-target", vec![second.clone()]);
    until(cx, "collision options", |cx| {
        cx.debug_bounds("upload-replace").is_some() && cx.debug_bounds("upload-keep").is_some()
    });
    click(cx, "upload-keep");
    until(cx, "both files", |_| {
        std::fs::read(root.join("target/two (2).txt"))
            .ok()
            .as_deref()
            == Some(b"changed")
    });
    assert_eq!(
        std::fs::read(root.join("target/two.txt")).unwrap(),
        b"second file"
    );
    drop_files(cx, "file-row-target", vec![second.clone()]);
    until(cx, "replace option", |cx| {
        cx.debug_bounds("upload-replace").is_some()
    });
    click(cx, "upload-replace");
    until(cx, "replacement", |_| {
        std::fs::read(root.join("target/two.txt")).ok().as_deref() == Some(b"changed")
    });
    drop_files(cx, "file-row-locked", vec![first.clone()]);
    until(cx, "node permission error", |cx| {
        cx.debug_bounds("files-error-Permission denied").is_some()
    });
    menu_on(cx, "file-row-target");
    click(cx, "file-menu-upload");
    assert!(cx.did_prompt_for_paths());
    let third = sources.join("picked.txt");
    std::fs::write(&third, b"picked").unwrap();
    cx.simulate_path_prompt_response(move |options| {
        assert!(options.files && options.multiple && !options.directories);
        Some(vec![third])
    });
    until(cx, "picker upload", |_| {
        std::fs::read(root.join("target/picked.txt"))
            .ok()
            .as_deref()
            == Some(b"picked")
    });
    drop(running);
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(sources).unwrap();
}

#[gpui::test]
fn file_menu_paths_pins_download_and_refresh(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-menu-journey");
    let downloads = parvi_testkit::fresh_dir("files-menu-downloads");
    std::fs::create_dir(root.join("folder")).unwrap();
    let bytes = b"\x00\xffdownload\n";
    std::fs::write(root.join("folder/note.bin"), bytes).unwrap();
    let running = start(root.clone());
    let settings = root.join("app.json");
    std::fs::write(
        &settings,
        serde_json::to_vec(&serde_json::json!({"files":{"download_folder":downloads}})).unwrap(),
    )
    .unwrap();
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let saved = settings.clone();
    let test_cx = cx;
    let (_, cx) = test_cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), Some(saved), w, cx)
    });
    browse_root(cx);
    until(cx, "folder", |cx| {
        cx.debug_bounds("file-row-folder").is_some()
    });
    click(cx, "file-row-folder");
    until(cx, "file", |cx| {
        cx.debug_bounds("file-row-note.bin").is_some()
    });
    menu_on(cx, "file-row-note.bin");
    click(cx, "file-menu-copy-path");
    assert_eq!(
        cx.read_from_clipboard().and_then(|x| x.text()),
        Some(root.join("folder/note.bin").to_string_lossy().into_owned())
    );
    menu_on(cx, "file-row-note.bin");
    click(cx, "file-menu-copy-relative");
    assert_eq!(
        cx.read_from_clipboard().and_then(|x| x.text()).as_deref(),
        Some("folder/note.bin")
    );
    menu_on(cx, "file-row-note.bin");
    click(cx, "file-menu-copy-folder");
    assert_eq!(
        cx.read_from_clipboard().and_then(|x| x.text()),
        Some(root.join("folder").to_string_lossy().into_owned())
    );
    menu_on(cx, "file-row-note.bin");
    click(cx, "file-menu-parent");
    until(cx, "selected file", |cx| {
        cx.debug_bounds("selected-note.bin").is_some()
    });
    menu_on(cx, "file-row-note.bin");
    click(cx, "file-menu-pin");
    until(cx, "pin shown", |cx| {
        cx.debug_bounds("files-pin-0").is_some()
    });
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "pin saved", |_| {
        std::fs::read(&settings)
            .ok()
            .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok())
            .is_some_and(|v| {
                v["files"]["pins"]
                    .as_array()
                    .is_some_and(|pins| pins.len() == 1)
            })
    });
    menu_on(cx, "file-row-note.bin");
    click(cx, "file-menu-download");
    let destination = downloads.join("note.bin");
    let notice: &'static str =
        Box::leak(format!("files-notice-Downloaded to {}", destination.display()).into_boxed_str());
    until(cx, "download and destination notice", |cx| {
        std::fs::read(&destination).ok().as_deref() == Some(bytes)
            && cx.debug_bounds(notice).is_some()
    });
    cx.executor().advance_clock(Duration::from_secs(6));
    until(cx, "download notice expires", |cx| {
        cx.debug_bounds(notice).is_none()
    });
    std::fs::write(root.join("folder/new.txt"), "new").unwrap();
    menu_on(cx, "file-row-note.bin");
    click(cx, "file-menu-refresh");
    until(cx, "new file after refresh", |cx| {
        cx.debug_bounds("file-row-new.txt").is_some()
    });
    let url = running.url.clone();
    let saved = settings.clone();
    let (rebuilt, again) = test_cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), Some(saved), w, cx)
    });
    until(again, "pin after rebuild", |cx| {
        cx.debug_bounds("files-pin-0").is_some()
            && rebuilt.read_with(cx, |s, cx| {
                !parvi_app_kit::Host::new(s.hub()).nodes(cx).is_empty()
            })
    });
    click(again, "files-pin-0");
    until(again, "pinned file opened", |cx| {
        cx.debug_bounds("file-view-note.bin").is_some()
            && cx.debug_bounds("file-row-note.bin").is_some()
    });
    menu_on(again, "file-row-note.bin");
    click(again, "file-menu-pin");
    until(again, "unpinned", |cx| {
        cx.debug_bounds("files-pin-0").is_none()
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(downloads).unwrap();
}

#[gpui::test]
fn page_renders_node_data(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-page");
    std::fs::write(root.join("note.txt"), "hello").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let settings = root.join("app.json");
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_files_app::build],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    until(cx, "Files page", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    browse_root(cx);
    until(cx, "node listing", |cx| {
        cx.debug_bounds("file-row-note.txt").is_some()
    });
    let at = cx.debug_bounds("file-row-note.txt").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    until(cx, "file viewer", |cx| {
        cx.debug_bounds("files-edit").is_some()
    });
    let viewer = cx.debug_bounds("files-viewer").unwrap();
    let page = cx.debug_bounds("files-page").unwrap();
    assert!(
        viewer.size.width >= page.size.width / 2.,
        "viewer should use at least half the window"
    );
    let pane = cx.debug_bounds("files-pane").unwrap();
    for id in ["files-create-file", "files-create-folder", "files-upload"] {
        let action = cx.debug_bounds(id).expect("action drawn");
        assert!(
            action.right() <= pane.right() && action.bottom() <= pane.bottom(),
            "{id} stays inside its pane"
        );
    }
    menu_on(cx, "file-row-note.txt");
    assert!(
        cx.debug_bounds("file-view-note.txt").is_some(),
        "right-click keeps the viewer"
    );
    click(cx, "file-menu-copy-path");
    let edit = cx.debug_bounds("files-edit").unwrap().center();
    cx.simulate_click(edit, Modifiers::none());
    until(cx, "editing", |cx| cx.debug_bounds("files-save").is_some());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("edited");
    cx.simulate_keystrokes("ctrl-s");
    until(cx, "saved text", |_| {
        std::fs::read(root.join("note.txt")).ok().as_deref() == Some(b"edited")
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// Revealing either kind of path opens its actual location, including a root
/// and a folder whose name looks like a media file.
#[gpui::test]
fn reveal_opens_files_and_lists_folders(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-reveal");
    let folder = root.join("clips.pdf");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("note.txt"), "hello reveal").unwrap();
    std::fs::write(root.join("outside.txt"), "outside").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[toy, parvi_files_app::build], Some(url), None, window, cx)
    });
    until(cx, "node linked", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == parvi_app_kit::link::client::LinkState::Open
        })
    });
    let host = shell.read_with(cx, |s, _| {
        parvi_app_kit::Host::new(s.hub()).for_page("toy", &[])
    });
    let reveal = |path: &std::path::Path| parvi_app_kit::Ask::Reveal {
        what: "path".into(),
        id: path.to_string_lossy().into_owned(),
        node: Some(parvi_core::NodeId(running.node.clone())),
        part: None,
        beside: false,
    };
    cx.update(|_, cx| host.ask(reveal(&folder.join("note.txt")), cx).unwrap());
    until(cx, "revealed file selected and open", |cx| {
        cx.debug_bounds("selected-note.txt").is_some()
            && cx.debug_bounds("file-view-note.txt").is_some()
            && cx.debug_bounds("files-edit").is_some()
    });
    assert!(cx.debug_bounds("file-row-outside.txt").is_none());
    cx.update(|_, cx| host.ask(reveal(&root), cx).unwrap());
    until(cx, "revealed root contents", |cx| {
        cx.debug_bounds("file-row-outside.txt").is_some()
            && cx.debug_bounds("file-row-clips.pdf").is_some()
    });
    cx.update(|_, cx| host.ask(reveal(&folder), cx).unwrap());
    until(cx, "revealed folder contents", |cx| {
        cx.debug_bounds("file-row-note.txt").is_some()
            && cx.debug_bounds("file-row-outside.txt").is_none()
    });
    assert!(cx.debug_bounds("file-view-clips.pdf").is_none());
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// Carries the old editor's discard/keep interaction through the node.
#[gpui::test]
fn editing_by_keyboard_keeps_the_work_and_discard_restores_it(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-draft");
    std::fs::write(root.join("draft.txt"), "original").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, window, cx)
    });
    until(cx, "Files", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    browse_root(cx);
    until(cx, "listing", |cx| {
        cx.debug_bounds("file-row-draft.txt").is_some()
    });
    click(cx, "file-row-draft.txt");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("unsaved");
    cx.simulate_keystrokes("escape");
    until(cx, "discard question", |cx| {
        cx.debug_bounds("files-keep-editing").is_some()
    });
    click(cx, "files-keep-editing");
    cx.simulate_keystrokes("escape");
    until(cx, "discard question again", |cx| {
        cx.debug_bounds("files-confirm-discard").is_some()
    });
    click(cx, "files-confirm-discard");
    until(cx, "read mode", |cx| {
        cx.debug_bounds("files-edit").is_some()
    });
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-a ctrl-c");
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("original")
    );
    assert_eq!(std::fs::read(root.join("draft.txt")).unwrap(), b"original");
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// A file's draft survives opening another folder and returning to it.
#[gpui::test]
fn opening_a_folder_keeps_the_previous_files_draft(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-draft-navigation");
    std::fs::write(root.join("draft.txt"), "original").unwrap();
    std::fs::create_dir(root.join("folder")).unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, window, cx)
    });
    until(cx, "Files", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    browse_root(cx);
    until(cx, "listing", |cx| {
        cx.debug_bounds("file-row-draft.txt").is_some()
    });
    click(cx, "file-row-draft.txt");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("kept draft");
    click(cx, "file-row-folder");
    until(cx, "folder open", |cx| {
        cx.debug_bounds("file-row-draft.txt").is_none()
    });
    click(cx, "files-up");
    until(cx, "back in root", |cx| {
        cx.debug_bounds("file-row-draft.txt").is_some()
    });
    click(cx, "file-row-draft.txt");
    until(cx, "reopened", |cx| cx.debug_bounds("files-edit").is_some());
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-s");
    until(cx, "draft saved", |_| {
        std::fs::read(root.join("draft.txt")).ok().as_deref() == Some(b"kept draft")
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// Media is shown immediately, without reading its bytes first.
#[gpui::test]
fn sound_video_and_pdf_open_to_play_without_reading(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-media");
    for name in ["clip.mp4", "voice.mp3", "paper.pdf"] {
        std::fs::write(root.join(name), b"").unwrap();
    }
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, window, cx)
    });
    until(cx, "Files", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    browse_root(cx);
    for name in ["clip.mp4", "voice.mp3", "paper.pdf"] {
        let id = format!("file-row-{name}");
        until(cx, "media row", |cx| {
            cx.debug_bounds(Box::leak(id.clone().into_boxed_str()))
                .is_some()
        });
        std::fs::remove_file(root.join(name)).unwrap();
        click(cx, Box::leak(id.into_boxed_str()));
        until(cx, "media open action", |cx| {
            cx.debug_bounds("files-play").is_some()
        });
    }
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// The table's navigation keys select paths, open files and return to the table.
#[gpui::test]
fn the_table_takes_the_key_map(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-keys");
    std::fs::write(root.join("a.txt"), "a").unwrap();
    std::fs::write(root.join("z.txt"), "z").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    until(cx, "Files", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    browse_root(cx);
    until(cx, "rows", |cx| cx.debug_bounds("file-row-z.txt").is_some());
    cx.simulate_keystrokes("down end ctrl-shift-c");
    let text = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(text.as_deref(), root.join("z.txt").to_str());
    cx.simulate_keystrokes("enter");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    cx.simulate_keystrokes("escape");
    until(cx, "viewer closed", |cx| {
        cx.debug_bounds("files-edit").is_none()
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// A key that acts closes the menu before acting, as does Escape.
#[gpui::test]
fn a_key_that_acts_closes_the_menu(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-menu");
    std::fs::write(root.join("note.txt"), "text").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    until(cx, "Files", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    browse_root(cx);
    until(cx, "row", |cx| {
        cx.debug_bounds("file-row-note.txt").is_some()
    });
    let at = cx.debug_bounds("file-row-note.txt").unwrap().center();
    cx.simulate_mouse_down(at, gpui::MouseButton::Right, Modifiers::none());
    until(cx, "menu", |cx| cx.debug_bounds("file-menu-open").is_some());
    cx.simulate_keystrokes("down");
    until(cx, "menu closed", |cx| {
        cx.debug_bounds("file-menu-open").is_none()
    });
    cx.simulate_mouse_down(at, gpui::MouseButton::Right, Modifiers::none());
    until(cx, "menu again", |cx| {
        cx.debug_bounds("file-menu-open").is_some()
    });
    cx.simulate_keystrokes("escape");
    until(cx, "menu dismissed", |cx| {
        cx.debug_bounds("file-menu-open").is_none()
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn a_reload_keeps_the_pane_on_its_device(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-reload");
    let running = start(root.clone());
    let settings = root.join("settings.json");
    let saved = serde_json::json!({"files":{"pane":
        {"node":"away-node","path":"/example/models","selected":"m.glb"}
    }});
    std::fs::write(&settings, serde_json::to_vec(&saved).unwrap()).unwrap();
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), Some(settings), w, cx)
    });
    until(cx, "Files", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    until(cx, "away pane", |cx| cx.debug_bounds("pane-away").is_some());
    let kept = shell.read_with(cx, |s, cx| {
        s.hub()
            .read(cx)
            .settings
            .section::<parvi_files_app::store::Kept>("files")
            .unwrap()
    });
    assert_eq!(kept.pane.node, "away-node");
    assert_eq!(kept.pane.path.as_deref(), Some("/example/models"));
    assert_eq!(kept.pane.selected.as_deref(), Some("m.glb"));
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

struct TerminalPage {
    host: parvi_app_kit::Host,
}
impl parvi_app_kit::Page for TerminalPage {
    const ID: &'static str = "terminals";
    const TITLE: &'static str = "Terminals";
}
impl gpui::Render for TerminalPage {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::prelude::*;
        gpui::div().size_full().children(self.host.beside(cx))
    }
}
fn terminal(
    host: parvi_app_kit::Host,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> parvi_app_kit::Mounted {
    parvi_app_kit::mount(host, window, cx, |host, _, _| TerminalPage { host })
}

/// A long rendered document stays in the terminal's side pane and scrolls.
#[gpui::test]
fn markdown_beside_terminal_scrolls_inside_its_border(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-beside-markdown");
    let file = root.join("long.md");
    std::fs::write(
        &file,
        (0..120)
            .map(|n| format!("Paragraph {n}: text in a long document.\n\n"))
            .collect::<String>(),
    )
    .unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[terminal, parvi_files_app::build], Some(url), None, w, cx)
    });
    until(cx, "node linked", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == parvi_app_kit::link::client::LinkState::Open
        })
    });
    let host = shell.read_with(cx, |s, _| {
        parvi_app_kit::Host::new(s.hub()).for_page("terminals", &[])
    });
    cx.update(|_, cx| {
        host.ask(
            parvi_app_kit::Ask::OpenPath {
                node: parvi_core::NodeId(running.node.clone()),
                path: file.to_string_lossy().into_owned(),
                beside: true,
            },
            cx,
        )
        .unwrap()
    });
    until(cx, "rendered Markdown", |cx| {
        cx.debug_bounds("files-markdown-rendered").is_some()
    });
    let viewer = cx.debug_bounds("files-viewer-bounds").unwrap();
    let viewport = cx.debug_bounds("files-markdown-rendered").unwrap();
    let before = cx.debug_bounds("files-markdown-content").unwrap();
    assert!(viewport.top() >= viewer.top() && viewport.bottom() <= viewer.bottom());
    assert!(
        before.size.height > viewport.size.height,
        "document exceeds the viewport"
    );
    cx.simulate_event(gpui::ScrollEvent {
        position: viewport.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-500.))),
        modifiers: Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
        source: gpui::ScrollSource::Pointer,
    });
    cx.run_until_parked();
    let after = cx.debug_bounds("files-markdown-content").unwrap();
    assert!(
        after.top() < before.top(),
        "scroll moves the document within its viewport"
    );
    let viewport_after = cx.debug_bounds("files-markdown-rendered").unwrap();
    assert_eq!(viewport, viewport_after);
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// A beside ask leaves the terminal shown and switching files keeps the draft.
#[gpui::test]
fn open_path_beside_keeps_the_terminal_and_the_draft(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-beside");
    std::fs::write(root.join("first.txt"), "first").unwrap();
    std::fs::write(root.join("second.txt"), "second").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[terminal, parvi_files_app::build], Some(url), None, w, cx)
    });
    until(cx, "terminal shown", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("terminals")
    });
    until(cx, "node linked", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == parvi_app_kit::link::client::LinkState::Open
        })
    });
    let host = shell.read_with(cx, |s, _| {
        parvi_app_kit::Host::new(s.hub()).for_page("terminals", &[])
    });
    let ask = |path: std::path::PathBuf| parvi_app_kit::Ask::OpenPath {
        node: parvi_core::NodeId(running.node.clone()),
        path: path.to_string_lossy().into_owned(),
        beside: true,
    };
    cx.update(|_, cx| host.ask(ask(root.join("first.txt")), cx).unwrap());
    until(cx, "first viewer", |cx| {
        cx.debug_bounds("file-view-first.txt").is_some() && cx.debug_bounds("files-edit").is_some()
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.shown_page()),
        Some("terminals")
    );
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("draft");
    let unsaved = shell.read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx));
    assert!(
        unsaved
            .iter()
            .any(|(page, label)| *page == "files" && label.contains("first.txt")),
        "{unsaved:?}"
    );
    assert!(!cx.simulate_close(), "the beside draft holds the window");
    until(cx, "close confirmation", |cx| {
        cx.debug_bounds("close-stay").is_some()
    });
    click(cx, "close-stay");
    cx.update(|window, cx| shell.update(cx, |shell, cx| shell.show(0, window, cx)));
    until(cx, "terminal restored", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("terminals")
    });
    cx.update(|_, cx| host.ask(ask(root.join("second.txt")), cx).unwrap());
    until(cx, "second viewer", |cx| {
        cx.debug_bounds("file-view-second.txt").is_some() && cx.debug_bounds("files-edit").is_some()
    });
    cx.update(|_, cx| host.ask(ask(root.join("first.txt")), cx).unwrap());
    until(cx, "first again", |cx| {
        cx.debug_bounds("file-view-first.txt").is_some() && cx.debug_bounds("files-edit").is_some()
    });
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-s");
    until(cx, "draft saved", |_| {
        std::fs::read(root.join("first.txt")).ok().as_deref() == Some(b"draft")
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// Name search reaches into a folder and opens a result from the node.
#[gpui::test]
fn find_updates_as_typed_shows_paths_lines_and_truncation(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-find-live");
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/readme.md"), "read me").unwrap();
    std::fs::write(root.join("sub/realm.txt"), "other").unwrap();
    let text = format!(
        "{}line marker\n{}",
        "blank\n".repeat(11),
        "tail\n".repeat(80)
    );
    std::fs::write(root.join("sub/a.rs"), text).unwrap();
    for n in 0..55 {
        std::fs::write(root.join(format!("capword-{n:02}.txt")), "x").unwrap();
    }
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[observed_files], Some(url), None, w, cx)
    });
    browse_root(cx);
    until(cx, "folder", |cx| cx.debug_bounds("file-row-sub").is_some());
    let page = SEARCH_PAGE.with(|page| page.borrow().as_ref().unwrap().clone());
    cx.simulate_keystrokes("/");
    cx.simulate_input("rea");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "first results", |cx| {
        cx.debug_bounds("find-label-sub/readme.md").is_some()
            && cx.debug_bounds("find-label-sub/realm.txt").is_some()
    });
    let old_revision = page.read_with(cx, |p, _| p.side.pane.find_revision);
    cx.simulate_input("d");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "new results", |cx| {
        cx.debug_bounds("find-label-sub/readme.md").is_some()
            && cx.debug_bounds("find-label-sub/realm.txt").is_none()
    });
    page.update(cx, |p, cx| {
        p.side.pane.found(
            old_revision,
            Ok(parvi_files_contract::Matches {
                matches: vec![parvi_files_contract::Match {
                    path: "/stale".into(),
                    name: "stale".into(),
                    line: None,
                    text: None,
                }],
                truncated: false,
            }),
        );
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("find-label-sub/readme.md").is_some());
    assert!(cx.debug_bounds("find-result-stale").is_none());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("line marker");
    click(cx, "search-mode");
    until(cx, "content result", |cx| {
        cx.debug_bounds("find-label-sub/a.rs:12").is_some()
    });
    click(cx, "find-result-a.rs");
    until(cx, "line 12 selected, lines above it in view", |cx| {
        page.read_with(cx, |p, cx| {
            let code = p.side.code.read(cx);
            code.selected_lines() == Some((12, 12)) && code.top_line() == 8
        })
    });
    cx.simulate_keystrokes("escape / ctrl-a");
    cx.simulate_input("capword");
    click(cx, "search-mode");
    until(cx, "truncation note", |cx| {
        cx.debug_bounds("files-find-truncated").is_some()
    });
    click(cx, "files-search");
    cx.simulate_keystrokes("ctrl-a backspace");
    until(cx, "the folder again", |cx| {
        cx.debug_bounds("file-row-sub").is_some()
    });
    click(cx, "file-row-sub");
    until(cx, "into sub", |cx| {
        cx.debug_bounds("file-row-a.rs").is_some()
    });
    click(cx, "files-search");
    cx.simulate_input("rea");
    click(cx, "files-up");
    until(cx, "a new folder starts with an empty search", |cx| {
        page.read_with(cx, |p, cx| p.side.search.read(cx).text.is_empty())
            && cx.debug_bounds("file-row-sub").is_some()
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn find_opens_a_file_below_the_current_folder(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-find");
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(root.join("nested/needle.txt"), "found").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    until(cx, "Files", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    browse_root(cx);
    until(cx, "folder", |cx| {
        cx.debug_bounds("file-row-nested").is_some()
    });
    cx.simulate_keystrokes("/");
    cx.simulate_input("needle");
    cx.simulate_keystrokes("enter");
    until(cx, "search result", |cx| {
        cx.debug_bounds("find-result-needle.txt").is_some()
    });
    menu_on(cx, "find-result-needle.txt");
    click(cx, "file-menu-copy-path");
    assert_eq!(
        cx.read_from_clipboard().and_then(|v| v.text()),
        Some(
            root.join("nested/needle.txt")
                .to_string_lossy()
                .into_owned()
        )
    );
    until(cx, "matched file in its folder", |cx| {
        cx.debug_bounds("selected-needle.txt").is_some()
    });
    click(cx, "file-row-needle.txt");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    cx.simulate_keystrokes("escape / ctrl-a");
    cx.simulate_input("found");
    click(cx, "search-mode");
    until(cx, "content result", |cx| {
        cx.debug_bounds("find-result-needle.txt").is_some()
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// A stale save keeps both the outside change and the operator's draft.
#[gpui::test]
fn conflicting_save_can_write_a_separate_copy(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-conflict");
    std::fs::write(root.join("note.txt"), "before").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    until(cx, "Files", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    browse_root(cx);
    until(cx, "file", |cx| {
        cx.debug_bounds("file-row-note.txt").is_some()
    });
    click(cx, "file-row-note.txt");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("my draft");
    std::fs::write(root.join("note.txt"), "other edit").unwrap();
    cx.simulate_keystrokes("ctrl-s");
    until(cx, "conflict", |cx| {
        cx.debug_bounds("conflict-copy").is_some()
    });
    click(cx, "conflict-copy");
    until(cx, "copy saved", |_| {
        std::fs::read(root.join("note (mine).txt")).ok().as_deref() == Some(b"my draft")
    });
    until(cx, "saved copy appears in the list", |cx| {
        cx.debug_bounds("file-row-note (mine).txt").is_some()
    });
    assert_eq!(std::fs::read(root.join("note.txt")).unwrap(), b"other edit");
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// Unsaved edits in the main Files page hold the app window open.
#[gpui::test]
fn unsaved_file_holds_the_window_until_the_operator_decides(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-close");
    std::fs::write(root.join("note.txt"), "before").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    until(cx, "Files", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    browse_root(cx);
    until(cx, "row", |cx| {
        cx.debug_bounds("file-row-note.txt").is_some()
    });
    click(cx, "file-row-note.txt");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("unsaved");
    assert!(!cx.simulate_close(), "the unsaved draft refuses close");
    until(cx, "close confirmation", |cx| {
        cx.debug_bounds("close-stay").is_some()
    });
    click(cx, "close-stay");
    assert!(!cx.simulate_close(), "keeping the edit still refuses close");
    assert_eq!(std::fs::read(root.join("note.txt")).unwrap(), b"before");
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// Picture zoom is shared and kept in the Files settings.
#[cfg(unix)]
#[gpui::test]
fn read_markdown_json_text_binary_and_permission_error(cx: &mut TestAppContext) {
    use parvi_files_app::open::Show;
    use std::os::unix::fs::PermissionsExt;
    let root = parvi_testkit::fresh_dir("files-read-finish");
    let downloads = parvi_testkit::fresh_dir("files-read-downloads");
    std::fs::write(root.join("README.md"), "# Heading\nBody\n").unwrap();
    std::fs::write(root.join("data.json"), "{\"a\":1}").unwrap();
    std::fs::write(root.join("lines.txt"), "first\nsecond\n").unwrap();
    std::fs::write(root.join("data.bin"), [0, 255, 32]).unwrap();
    std::fs::write(root.join("secret.txt"), "secret").unwrap();
    std::fs::set_permissions(
        root.join("secret.txt"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let settings = root.join("settings.json");
    std::fs::write(
        &settings,
        serde_json::to_vec(&serde_json::json!({"files":{"download_folder":downloads}})).unwrap(),
    )
    .unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[observed_files], Some(url), Some(settings), w, cx)
    });
    browse_root(cx);
    until(cx, "markdown", |cx| {
        cx.debug_bounds("file-row-README.md").is_some()
    });
    let page = SEARCH_PAGE.with(|p| p.borrow().as_ref().unwrap().clone());
    click(cx, "file-row-README.md");
    until(cx, "rendered markdown", |cx| {
        cx.debug_bounds("files-source").is_some()
            && cx.debug_bounds("files-markdown-rendered").is_some()
            && page.read_with(cx, |p, _| {
                p.side.opened.as_ref().is_some_and(|o| {
                    matches!(o.show, Show::Text { markdown: Some(_) }) && !o.source
                })
            })
    });
    click(cx, "files-source");
    assert!(page.read_with(cx, |p, _| p.side.opened.as_ref().unwrap().source));
    assert!(cx.debug_bounds("files-code-top-1").is_some());
    click(cx, "file-row-data.json");
    until(cx, "JSON as written", |cx| {
        cx.debug_bounds("files-json-format").is_some()
            && cx.debug_bounds("files-code-top-1").is_some()
    });
    assert_eq!(
        page.read_with(cx, |p, cx| p.side.code.read(cx).text().to_owned()),
        "{\"a\":1}"
    );
    click(cx, "files-json-format");
    until(cx, "formatted JSON", |cx| {
        cx.debug_bounds("files-json-formatted").is_some()
    });
    assert_eq!(
        page.read_with(cx, |p, cx| p.side.pretty_code.read(cx).text().to_owned()),
        "{\n  \"a\": 1\n}"
    );
    click(cx, "files-edit");
    assert_eq!(
        page.read_with(cx, |p, cx| p.side.code.read(cx).text().to_owned()),
        "{\"a\":1}"
    );
    cx.simulate_keystrokes("escape");
    click(cx, "file-row-lines.txt");
    until(cx, "text view", |cx| {
        page.read_with(cx, |p, cx| {
            p.side
                .opened
                .as_ref()
                .is_some_and(|o| o.name == "lines.txt" && matches!(o.show, Show::Text { .. }))
                && p.side.code.read(cx).text() == "first\nsecond\n"
        })
    });
    click(cx, "file-row-data.bin");
    until(cx, "binary download", |cx| {
        cx.debug_bounds("viewer-download").is_some()
    });
    click(cx, "viewer-download");
    let destination = downloads.join("data.bin");
    let notice: &'static str =
        Box::leak(format!("files-notice-Downloaded to {}", destination.display()).into_boxed_str());
    until(cx, "binary downloaded", |cx| {
        std::fs::read(&destination).ok().as_deref() == Some(&[0, 255, 32])
            && cx.debug_bounds(notice).is_some()
    });
    click(cx, "file-row-secret.txt");
    until(cx, "permission denied", |cx| {
        page.read_with(cx, |p, _| p.side.opened.as_ref().is_some_and(|o| matches!(&o.show, Show::Failed(message) if message.contains("Permission denied"))))
    });
    drop(running);
    std::fs::set_permissions(
        root.join("secret.txt"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(downloads).unwrap();
}

#[gpui::test]
fn create_rename_delete_and_diff_in_the_files_page(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-act-finish");
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    std::fs::write(root.join("tracked.txt"), "before\n").unwrap();
    std::fs::write(root.join("clean.txt"), "unchanged\n").unwrap();
    git(&["add", "tracked.txt", "clean.txt"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-qm",
        "initial",
    ]);
    std::fs::write(root.join("tracked.txt"), "after\n").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[observed_files], Some(url), None, w, cx)
    });
    browse_root(cx);
    let page = SEARCH_PAGE.with(|p| p.borrow().as_ref().unwrap().clone());
    until(cx, "tracked file", |cx| {
        cx.debug_bounds("file-row-tracked.txt").is_some()
    });
    click(cx, "file-row-clean.txt");
    until(cx, "clean diff action", |cx| {
        cx.debug_bounds("files-diff").is_some()
    });
    click(cx, "files-diff");
    until(cx, "empty diff message", |cx| {
        cx.debug_bounds("files-diff-empty").is_some()
    });
    click(cx, "file-row-tracked.txt");
    until(cx, "diff action", |cx| {
        cx.debug_bounds("files-diff").is_some()
    });
    click(cx, "files-diff");
    until(cx, "diff against HEAD", |cx| {
        page.read_with(cx, |p, _| {
            p.side
                .opened
                .as_ref()
                .and_then(|o| o.diff.as_ref())
                .is_some_and(|d| d.as_ref().is_ok_and(|d| d.diff.contains("-before\n+after")))
        })
    });
    menu_on(cx, "file-row-tracked.txt");
    click(cx, "file-menu-rename");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("renamed.txt");
    cx.simulate_keystrokes("enter");
    until(cx, "open file follows rename", |cx| {
        root.join("renamed.txt").is_file()
            && cx.debug_bounds("file-view-renamed.txt").is_some()
            && cx.debug_bounds("file-row-renamed.txt").is_some()
            && cx.debug_bounds("file-row-tracked.txt").is_none()
            && cx.debug_bounds("files-name").is_none()
    });
    click(cx, "files-create-file");
    cx.simulate_input("new.txt");
    cx.simulate_keystrokes("enter");
    until(cx, "new file", |cx| {
        root.join("new.txt").is_file() && cx.debug_bounds("file-row-new.txt").is_some()
    });
    click(cx, "files-create-folder");
    cx.simulate_input("new-folder");
    click(cx, "files-name-apply");
    until(cx, "new folder", |cx| {
        root.join("new-folder").is_dir() && cx.debug_bounds("file-row-new-folder").is_some()
    });
    click(cx, "files-create-file");
    cx.simulate_input("abandoned.txt");
    cx.simulate_keystrokes("escape");
    until(cx, "naming cancelled", |cx| {
        cx.debug_bounds("files-name").is_none()
    });
    assert!(!root.join("abandoned.txt").exists());
    click(cx, "file-row-new.txt");
    until(cx, "new file opens", |cx| {
        cx.debug_bounds("file-view-new.txt").is_some()
    });
    menu_on(cx, "file-row-new.txt");
    click(cx, "file-menu-delete");
    assert!(root.join("new.txt").exists());
    click(cx, "file-menu-delete");
    until(cx, "delete confirmed in the list and viewer", |cx| {
        !root.join("new.txt").exists()
            && cx.debug_bounds("file-row-new.txt").is_none()
            && cx.debug_bounds("file-view-new.txt").is_none()
            && page.read_with(cx, |p, _| p.side.opened.is_none())
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn picture_zoom_can_switch_between_fit_and_actual_size(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-picture");
    std::fs::write(root.join("pixel.png"), include_bytes!("pixel.png")).unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    until(cx, "Files", |cx| {
        shell.read_with(cx, |s, _| s.shown_page()) == Some("files")
    });
    browse_root(cx);
    until(cx, "picture", |cx| {
        cx.debug_bounds("file-row-pixel.png").is_some()
    });
    click(cx, "file-row-pixel.png");
    until(cx, "zoom controls", |cx| {
        cx.debug_bounds("zoom-1").is_some()
    });
    click(cx, "zoom-1");
    let zoom = shell.read_with(cx, |s, cx| {
        s.hub()
            .read(cx)
            .settings
            .section::<parvi_files_app::store::Kept>("files")
            .unwrap()
            .zoom
    });
    assert_eq!(zoom, Some(100));
    click(cx, "zoom-fit");
    let zoom = shell.read_with(cx, |s, cx| {
        s.hub()
            .read(cx)
            .settings
            .section::<parvi_files_app::store::Kept>("files")
            .unwrap()
            .zoom
    });
    assert_eq!(zoom, None);
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}
