//! The Files page draws a node's real listing.
use gpui::{Modifiers, ParentElement, TestAppContext, VisualTestContext};
use parvi_testkit::node::Node;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

thread_local! {
    static SENT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
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
        cx.debug_bounds("file-row-0-folder").is_some()
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    click(cx, "file-row-0-folder");
    until(cx, "dock file", |cx| {
        cx.debug_bounds("file-row-0-note.txt").is_some()
    });
    click(cx, "file-row-0-note.txt");
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
        cx.debug_bounds("file-row-0-note.txt").is_some() && cx.debug_bounds("files-send").is_some()
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
    until(cx, "folder", |cx| {
        cx.debug_bounds("file-row-0-folder").is_some()
    });
    click(cx, "file-row-0-folder");
    until(cx, "folder listing", |cx| {
        cx.debug_bounds("file-row-0-needle.txt").is_some()
    });
    cx.simulate_keystrokes("/");
    cx.simulate_input("needle");
    cx.simulate_keystrokes("enter");
    until(cx, "search result", |cx| {
        cx.debug_bounds("find-result-0-needle.txt").is_some()
    });
    click(cx, "find-result-0-needle.txt");
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
    until(cx, "folder rows", |cx| {
        cx.debug_bounds("file-row-0-target").is_some()
            && cx.debug_bounds("file-row-0-locked").is_some()
    });
    drop_files(cx, "file-row-0-target", vec![first.clone(), second.clone()]);
    until(cx, "two files uploaded", |_| {
        std::fs::read(root.join("target/one.bin")).ok().as_deref() == Some(&[0, 255, 128, 42])
            && std::fs::read(root.join("target/two.txt")).ok().as_deref() == Some(b"second file")
    });
    std::fs::write(&second, b"changed").unwrap();
    drop_files(cx, "file-row-0-target", vec![second.clone()]);
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
    drop_files(cx, "file-row-0-target", vec![second.clone()]);
    until(cx, "replace option", |cx| {
        cx.debug_bounds("upload-replace").is_some()
    });
    click(cx, "upload-replace");
    until(cx, "replacement", |_| {
        std::fs::read(root.join("target/two.txt")).ok().as_deref() == Some(b"changed")
    });
    drop_files(cx, "file-row-0-locked", vec![first.clone()]);
    until(cx, "node permission error", |cx| {
        cx.debug_bounds("files-error-Permission denied").is_some()
    });
    menu_on(cx, "file-row-0-target");
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
    until(cx, "folder", |cx| {
        cx.debug_bounds("file-row-0-folder").is_some()
    });
    click(cx, "file-row-0-folder");
    until(cx, "file", |cx| {
        cx.debug_bounds("file-row-0-note.bin").is_some()
    });
    menu_on(cx, "file-row-0-note.bin");
    click(cx, "file-menu-copy-relative");
    assert_eq!(
        cx.read_from_clipboard().and_then(|x| x.text()).as_deref(),
        Some("folder/note.bin")
    );
    menu_on(cx, "file-row-0-note.bin");
    click(cx, "file-menu-copy-folder");
    assert_eq!(
        cx.read_from_clipboard().and_then(|x| x.text()),
        Some(root.join("folder").to_string_lossy().into_owned())
    );
    menu_on(cx, "file-row-0-note.bin");
    click(cx, "file-menu-parent");
    until(cx, "selected file", |cx| {
        cx.debug_bounds("selected-0-note.bin").is_some()
    });
    menu_on(cx, "file-row-0-note.bin");
    click(cx, "file-menu-pin");
    until(cx, "pin shown", |cx| {
        cx.debug_bounds("files-pin-0-0").is_some()
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
    menu_on(cx, "file-row-0-note.bin");
    click(cx, "file-menu-download");
    let destination = downloads.join("note.bin");
    let notice: &'static str =
        Box::leak(format!("files-notice-Downloaded to {}", destination.display()).into_boxed_str());
    until(cx, "download and destination notice", |cx| {
        std::fs::read(&destination).ok().as_deref() == Some(bytes)
            && cx.debug_bounds(notice).is_some()
    });
    std::fs::write(root.join("folder/new.txt"), "new").unwrap();
    menu_on(cx, "file-row-0-note.bin");
    click(cx, "file-menu-refresh");
    until(cx, "new file after refresh", |cx| {
        cx.debug_bounds("file-row-0-new.txt").is_some()
    });
    let url = running.url.clone();
    let saved = settings.clone();
    let (rebuilt, again) = test_cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), Some(saved), w, cx)
    });
    until(again, "pin after rebuild", |cx| {
        cx.debug_bounds("files-pin-0-0").is_some()
            && rebuilt.read_with(cx, |s, cx| {
                !parvi_app_kit::Host::new(s.hub()).nodes(cx).is_empty()
            })
    });
    click(again, "files-pin-0-0");
    until(again, "pinned file opened", |cx| {
        cx.debug_bounds("file-view-note.bin").is_some()
            && cx.debug_bounds("file-row-0-note.bin").is_some()
    });
    menu_on(again, "file-row-0-note.bin");
    click(again, "file-menu-pin");
    until(again, "unpinned", |cx| {
        cx.debug_bounds("files-pin-0-0").is_none()
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
    until(cx, "node listing", |cx| {
        cx.debug_bounds("file-row-0-note.txt").is_some()
    });
    let at = cx.debug_bounds("file-row-0-note.txt").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    until(cx, "file viewer", |cx| {
        cx.debug_bounds("files-edit").is_some()
    });
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
    until(cx, "listing", |cx| {
        cx.debug_bounds("file-row-0-draft.txt").is_some()
    });
    click(cx, "file-row-0-draft.txt");
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
    until(cx, "listing", |cx| {
        cx.debug_bounds("file-row-0-draft.txt").is_some()
    });
    click(cx, "file-row-0-draft.txt");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("kept draft");
    click(cx, "file-row-0-folder");
    until(cx, "folder open", |cx| {
        cx.debug_bounds("file-row-0-draft.txt").is_none()
    });
    click(cx, "files-up-0");
    until(cx, "back in root", |cx| {
        cx.debug_bounds("file-row-0-draft.txt").is_some()
    });
    click(cx, "file-row-0-draft.txt");
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
    for name in ["clip.mp4", "voice.mp3", "paper.pdf"] {
        let id = format!("file-row-0-{name}");
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

/// A completed local copy refreshes the destination pane.
#[gpui::test]
fn a_finished_copy_is_selected_where_it_landed(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-copy");
    std::fs::write(root.join("note.txt"), "copied").unwrap();
    std::fs::create_dir(root.join("destination")).unwrap();
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
    until(cx, "destination", |cx| {
        cx.debug_bounds("file-row-1-destination").is_some()
    });
    click(cx, "file-row-1-destination");
    until(cx, "destination entered", |cx| {
        cx.debug_bounds("file-row-1-destination").is_none()
    });
    until(cx, "source", |cx| {
        cx.debug_bounds("file-row-0-note.txt").is_some()
    });
    click(cx, "file-row-0-note.txt");
    click(cx, "copy-0");
    until(cx, "copied file", |_| {
        std::fs::read(root.join("destination/note.txt"))
            .ok()
            .as_deref()
            == Some(b"copied")
    });
    until(cx, "copied file selected", |cx| {
        cx.debug_bounds("selected-1-note.txt").is_some()
    });
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
    until(cx, "rows", |cx| {
        cx.debug_bounds("file-row-0-z.txt").is_some()
    });
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
    until(cx, "row", |cx| {
        cx.debug_bounds("file-row-0-note.txt").is_some()
    });
    let at = cx.debug_bounds("file-row-0-note.txt").unwrap().center();
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

/// A kept pane on an unreachable node stays at its remembered location.
#[gpui::test]
fn a_reload_keeps_both_panes_on_their_devices(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-reload");
    std::fs::write(root.join("note.txt"), "text").unwrap();
    let running = start(root.clone());
    let settings = root.join("settings.json");
    let saved = serde_json::json!({"files":{"panes":[
        {"node":running.node,"path":root,"selected":"note.txt"},
        {"node":"away-node","path":"/example/models","selected":"m.glb"}
    ]}});
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
    until(cx, "away pane", |cx| {
        cx.debug_bounds("pane-away-1").is_some()
    });
    let kept = shell.read_with(cx, |s, cx| {
        s.hub()
            .read(cx)
            .settings
            .section::<parvi_files_app::store::Kept>("files")
            .unwrap()
    });
    assert_eq!(kept.panes[1].node, "away-node");
    assert_eq!(kept.panes[1].path.as_deref(), Some("/example/models"));
    assert_eq!(kept.panes[1].selected.as_deref(), Some("m.glb"));
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
    until(cx, "folder", |cx| {
        cx.debug_bounds("file-row-0-nested").is_some()
    });
    cx.simulate_keystrokes("/");
    cx.simulate_input("needle");
    cx.simulate_keystrokes("enter");
    until(cx, "search result", |cx| {
        cx.debug_bounds("find-result-0-needle.txt").is_some()
    });
    click(cx, "find-result-0-needle.txt");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    cx.simulate_keystrokes("escape / ctrl-a");
    cx.simulate_input("found");
    click(cx, "search-mode-0");
    until(cx, "content result", |cx| {
        cx.debug_bounds("find-result-0-needle.txt").is_some()
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
    until(cx, "file", |cx| {
        cx.debug_bounds("file-row-0-note.txt").is_some()
    });
    click(cx, "file-row-0-note.txt");
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
    until(cx, "row", |cx| {
        cx.debug_bounds("file-row-0-note.txt").is_some()
    });
    click(cx, "file-row-0-note.txt");
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
    until(cx, "picture", |cx| {
        cx.debug_bounds("file-row-0-pixel.png").is_some()
    });
    click(cx, "file-row-0-pixel.png");
    until(cx, "zoom controls", |cx| {
        cx.debug_bounds("zoom-1-0").is_some()
    });
    click(cx, "zoom-1-0");
    let zoom = shell.read_with(cx, |s, cx| {
        s.hub()
            .read(cx)
            .settings
            .section::<parvi_files_app::store::Kept>("files")
            .unwrap()
            .zoom
    });
    assert_eq!(zoom, Some(100));
    click(cx, "zoom-fit-0");
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
