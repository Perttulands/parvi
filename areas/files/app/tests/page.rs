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
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}
/// The menu on a row, from a right click.
fn menu_on(cx: &mut VisualTestContext, row: &'static str) {
    let at = cx.debug_bounds(row).expect("row").center();
    cx.simulate_mouse_down(at, gpui::MouseButton::Right, Modifiers::none());
    until(cx, "row menu", |cx| {
        cx.debug_bounds("menu-label-Copy path").is_some()
    });
}
/// Clicks a row of the open menu by its words.
fn menu(cx: &mut VisualTestContext, label: &str) {
    click(cx, leak(format!("menu-label-{label}")));
}
fn drop_files(cx: &mut VisualTestContext, row: &'static str, files: Vec<std::path::PathBuf>) {
    let position = cx.debug_bounds(row).expect("drop target").center();
    cx.simulate_event(gpui::FileDropEvent::Entered {
        position,
        paths: gpui::ExternalPaths(files.into_iter().collect()),
    });
    cx.simulate_event(gpui::FileDropEvent::Submit { position });
}
/// What the window's status says now.
fn status(shell: &gpui::Entity<parvi_app_shell::Shell>, cx: &mut VisualTestContext) -> String {
    shell.read_with(cx, |s, cx| {
        s.hub()
            .read(cx)
            .status
            .as_ref()
            .map(|s| s.text.clone())
            .unwrap_or_default()
    })
}
/// The page's tree, read: its first root open.
fn tree(cx: &mut VisualTestContext, row: &'static str) {
    until(cx, "the tree", |cx| cx.debug_bounds(row).is_some());
}
/// Lets the page's watch look once.
fn watch_once(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
}
fn kept(
    shell: &gpui::Entity<parvi_app_shell::Shell>,
    cx: &mut VisualTestContext,
) -> parvi_files_app::store::Kept {
    shell.read_with(cx, |s, cx| {
        s.hub()
            .read(cx)
            .settings
            .section::<parvi_files_app::store::Kept>("files")
            .unwrap_or_default()
    })
}
fn modified(path: &std::path::Path, seconds_ago: u64) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(std::time::SystemTime::now() - Duration::from_secs(seconds_ago))
        .unwrap();
}

/// Alt+O opens the Files panel beside the terminals, from any page: the
/// same view a printed path opens, which browses, sends and remembers.
#[gpui::test]
fn files_panel_browses_sends_and_remembers_beside_the_terminals(cx: &mut TestAppContext) {
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
            &[toy, terminal, parvi_files_app::build],
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
    tree(cx, "file-row-folder");
    assert_eq!(
        shell.read_with(cx, |s, _| s.shown_page()),
        Some("terminals"),
        "the terminals come forward with the panel beside them"
    );
    assert!(cx.debug_bounds("dock-files").is_none(), "no dock column");
    assert!(cx.debug_bounds("files-close-beside").is_some(), "Close");
    click(cx, "file-row-folder");
    tree(cx, "file-row-note.txt");
    click(cx, "file-row-note.txt");
    until(cx, "the panel's viewer, its file read", |cx| {
        cx.debug_bounds("files-edit").is_some()
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
    cx.run_until_parked();
    assert!(cx.debug_bounds("file-view-note.txt").is_none(), "closed");
    assert!(!kept(&shell, cx).beside_open);
    cx.simulate_keystrokes("alt-o");
    until(cx, "remembered file", |cx| {
        cx.debug_bounds("file-view-note.txt").is_some() && cx.debug_bounds("files-send").is_some()
    });
    assert!(kept(&shell, cx).beside_open);
    click(cx, "files-back");
    until(cx, "back to the tree, the file picked", |cx| {
        cx.debug_bounds("picked-note.txt").is_some() && cx.debug_bounds("files-send").is_none()
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// The Files panel opens over the terminals at the focused session's
/// folder; a find there reaches names and text; a path the session prints
/// opens in the same panel.
#[cfg(unix)]
#[gpui::test]
fn terminal_panel_search_and_printed_path_open_beside(cx: &mut TestAppContext) {
    let Some(tmux) = parvi_testkit::tmux::Server::start_in("files-finish-terminal") else {
        assert!(std::env::var_os("PARVI_REQUIRE_TMUX").is_none());
        return;
    };
    let root = parvi_testkit::fresh_dir("files-finish-terminal-root");
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::create_dir(root.join("other")).unwrap();
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
    let row = leak(format!("row-{}::alpha", running.node));
    until(cx, "terminal session", |cx| cx.debug_bounds(row).is_some());
    click(cx, row);
    until(cx, "attach", |cx| cx.debug_bounds("peek-attach").is_some());
    click(cx, "peek-attach");
    let terminal = TERMINAL_PAGE.with(|p| p.borrow().as_ref().unwrap().clone());
    until(cx, "terminal painted", |cx| {
        terminal.read_with(cx, |p, _| p.geo.borrow()[0].origin.is_some())
    });
    // The panel opens at the session's folder, picked.
    cx.simulate_keystrokes("alt-o");
    until(cx, "the panel at the session's folder", |cx| {
        cx.debug_bounds("file-row-note.txt").is_some() && cx.debug_bounds("picked-sub").is_some()
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.shown_page()),
        Some("terminals")
    );
    assert!(
        cx.debug_bounds("file-row-other").is_some(),
        "the root shows"
    );
    click(cx, "files-search");
    cx.simulate_input("note");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "name hit", |cx| {
        cx.debug_bounds("hit-note.txt").is_some()
    });
    click(cx, "files-search");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("find marker");
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "a find in the text offered", |cx| {
        cx.debug_bounds("files-in-text").is_some()
    });
    click(cx, "files-in-text");
    until(cx, "content hit", |cx| {
        cx.debug_bounds("hit-note.txt:12").is_some()
    });
    click(cx, "hit-note.txt:12");
    until(cx, "the panel's file", |cx| {
        cx.debug_bounds("file-view-note.txt").is_some()
            && cx.debug_bounds("files-code-top-9").is_some()
    });
    cx.simulate_keystrokes("alt-o");
    until(cx, "the panel closed", |cx| {
        cx.debug_bounds("terminals-beside").is_none()
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

/// The whole job: find a file by name, edit it, save it, send it.
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
    let row = leak(format!("row-{}::alpha", running.node));
    until(cx, "session in rail", |cx| cx.debug_bounds(row).is_some());
    click(cx, row);
    until(cx, "attach session", |cx| {
        cx.debug_bounds("peek-attach").is_some()
    });
    click(cx, "peek-attach");
    click(cx, "tab-files");
    tree(cx, "file-row-folder");
    // Part of its name and Enter open it.
    cx.simulate_keystrokes("/");
    cx.simulate_input("needle");
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "search result", |cx| {
        cx.debug_bounds("hit-needle.txt").is_some()
    });
    cx.simulate_keystrokes("enter");
    until(cx, "viewer", |cx| {
        cx.debug_bounds("file-view-needle.txt").is_some() && cx.debug_bounds("files-edit").is_some()
    });
    click(cx, "files-edit");
    until(cx, "editor", |cx| cx.debug_bounds("files-save").is_some());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("after");
    cx.simulate_keystrokes("ctrl-s");
    until(cx, "saved file", |_| {
        std::fs::read(&file).ok().as_deref() == Some(b"after")
    });
    cx.simulate_keystrokes("escape");
    until(cx, "reading again", |cx| {
        cx.debug_bounds("files-send").is_some()
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

/// Many files changed in one folder stand as one Recent line; a click
/// opens it to those files, newest first, and each opens in one more. A
/// find from a folder under a hidden one covers that folder, and Esc goes
/// back to where the list was.
#[gpui::test]
fn a_recent_folder_opens_to_its_files_and_a_find_covers_where_you_stand(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-recent-folder");
    std::fs::create_dir_all(root.join("batch")).unwrap();
    std::fs::create_dir_all(root.join(".cache/walk")).unwrap();
    for n in 0..5u64 {
        let name = format!("batch/render-{n}.txt");
        std::fs::write(root.join(&name), &name).unwrap();
        modified(&root.join(&name), 100 + n * 10);
    }
    std::fs::write(root.join(".cache/walk/pic.txt"), "picture").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    until(cx, "the folder's line", |cx| {
        cx.debug_bounds("recent-batch/").is_some()
    });
    assert!(cx.debug_bounds("recent-render-0.txt").is_none());
    click(cx, "recent-batch/");
    until(cx, "the files changed in it", |cx| {
        cx.debug_bounds("recent-render-4.txt").is_some()
    });
    let newest = cx.debug_bounds("recent-render-0.txt").unwrap();
    let oldest = cx.debug_bounds("recent-render-4.txt").unwrap();
    assert!(newest.top() < oldest.top());
    click(cx, "recent-render-0.txt");
    until(cx, "one of them open", |cx| {
        cx.debug_bounds("file-view-render-0.txt").is_some()
    });
    // A second click on the line closes it.
    click(cx, "recent-batch/");
    until(cx, "the line closed", |cx| {
        cx.debug_bounds("recent-render-4.txt").is_none()
    });

    // Standing in a folder under a hidden one, a find covers it; from
    // elsewhere the hidden folder stays out.
    click(cx, "files-search");
    cx.simulate_input("pic");
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "nothing from elsewhere", |cx| {
        cx.debug_bounds("files-note-No file name has \u{201c}pic\u{201d}")
            .is_some()
    });
    cx.simulate_keystrokes("escape");
    let walk = root.join(".cache/walk");
    let host = shell.read_with(cx, |s, _| {
        parvi_app_kit::Host::new(s.hub()).for_page("files", &[])
    });
    let reveal = parvi_app_kit::Ask::Reveal {
        what: "path".into(),
        id: walk.to_string_lossy().into_owned(),
        node: None,
        part: None,
        beside: false,
    };
    cx.update(|_, cx| host.ask(reveal, cx).unwrap());
    until(cx, "the hidden folder shown", |cx| {
        cx.debug_bounds("picked-walk").is_some()
    });
    click(cx, "files-search");
    cx.simulate_input("pic");
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "the file beside you", |cx| {
        cx.debug_bounds("hit-pic.txt").is_some()
    });
    // Esc ends the find where it began: on the folder.
    cx.simulate_keystrokes("escape");
    until(cx, "back on the folder", |cx| {
        cx.debug_bounds("picked-walk").is_some() && cx.debug_bounds("hit-pic.txt").is_none()
    });
    // Enter pressed before the node has answered opens the first file
    // found, when it is found.
    click(cx, "files-search");
    cx.simulate_input("pic");
    cx.simulate_keystrokes("enter");
    assert!(cx.debug_bounds("file-view-pic.txt").is_none());
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "the file found opened by the early Enter", |cx| {
        cx.debug_bounds("file-view-pic.txt").is_some()
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// What changed most recently comes first, newest at the top, and opens
/// in one click; the files it names show where they are only when asked.
#[gpui::test]
fn recent_changes_come_first_and_open_in_one_click(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-recent");
    std::fs::create_dir_all(root.join("deep/down")).unwrap();
    std::fs::create_dir(root.join(".hidden")).unwrap();
    for (name, ago) in [
        ("old.txt", 3000),
        ("deep/down/report.md", 10),
        ("middle.txt", 100),
        (".hidden/secret.txt", 1),
    ] {
        std::fs::write(root.join(name), name).unwrap();
        modified(&root.join(name), ago);
    }
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    until(cx, "recent changes", |cx| {
        cx.debug_bounds("recent-report.md").is_some()
            && cx.debug_bounds("recent-middle.txt").is_some()
            && cx.debug_bounds("recent-old.txt").is_some()
    });
    let report = cx.debug_bounds("recent-report.md").unwrap();
    let middle = cx.debug_bounds("recent-middle.txt").unwrap();
    let old = cx.debug_bounds("recent-old.txt").unwrap();
    assert!(report.top() < middle.top() && middle.top() < old.top());
    assert!(
        cx.debug_bounds("recent-secret.txt").is_none(),
        "hidden folders stay out"
    );
    tree(cx, "file-row-deep");
    let folders = cx.debug_bounds("file-row-deep").unwrap();
    assert!(old.bottom() <= folders.top());
    // With nothing open the newest shows unasked, and is not kept as the
    // file to come back to.
    until(cx, "the newest file shown", |cx| {
        cx.debug_bounds("file-view-report.md").is_some()
    });
    assert_eq!(kept(&shell, cx).file, None);
    click(cx, "recent-report.md");
    until(cx, "opened from recent", |cx| {
        kept(&shell, cx).file.is_some()
    });
    assert!(
        cx.debug_bounds("file-row-report.md").is_none(),
        "opening from Recent leaves the tree as it was"
    );
    menu_on(cx, "recent-report.md");
    menu(cx, "Show in tree");
    until(cx, "shown in the tree", |cx| {
        cx.debug_bounds("picked-report.md").is_some()
    });
    assert_eq!(
        kept(&shell, cx).file.as_deref(),
        root.join("deep/down/report.md").to_str()
    );
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// What the page shows follows the disk while it is seen: a folder gains
/// a file, an open file reads again in place, an edit is never replaced.
#[gpui::test]
fn changes_on_disk_show_without_asking_and_never_replace_an_edit(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-live");
    std::fs::write(root.join("note.txt"), "first").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[observed_files], Some(url), None, w, cx)
    });
    tree(cx, "file-row-note.txt");
    // The page looks only while its window is the one in use.
    cx.update(|window, _| window.activate_window());
    let page = SEARCH_PAGE.with(|p| p.borrow().as_ref().unwrap().clone());
    click(cx, "file-row-note.txt");
    until(cx, "read", |cx| {
        page.read_with(cx, |p, cx| p.code.read(cx).text() == "first")
    });
    watch_once(cx);
    std::fs::write(root.join("arrived.txt"), "new").unwrap();
    std::fs::write(root.join("note.txt"), "second").unwrap();
    modified(&root.join("note.txt"), 0);
    until(cx, "the new file and the new text", |cx| {
        watch_once(cx);
        cx.debug_bounds("file-row-arrived.txt").is_some()
            && page.read_with(cx, |p, cx| p.code.read(cx).text() == "second")
    });
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("mine");
    std::fs::write(root.join("note.txt"), "third").unwrap();
    modified(&root.join("note.txt"), 0);
    until(cx, "told it changed under the edit", |cx| {
        watch_once(cx);
        cx.debug_bounds("files-changed-on-disk").is_some()
    });
    assert_eq!(
        page.read_with(cx, |p, cx| p.code.read(cx).text().to_owned()),
        "mine"
    );
    cx.simulate_keystrokes("ctrl-s");
    until(cx, "the save asks", |cx| {
        cx.debug_bounds("conflict-copy").is_some()
    });
    assert_eq!(std::fs::read(root.join("note.txt")).unwrap(), b"third");
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// A long log reads whole and opens at its end; it is too large to edit
/// here, and says so.
#[gpui::test]
fn a_long_log_opens_at_its_end(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-log");
    let log: String = (1..=60_000)
        .map(|n| format!("line {n} of the build\n"))
        .collect();
    assert!(log.len() > parvi_files_app::open::MAX_EDIT);
    std::fs::write(root.join("build.log"), &log).unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[observed_files], Some(url), None, w, cx)
    });
    tree(cx, "file-row-build.log");
    let page = SEARCH_PAGE.with(|p| p.borrow().as_ref().unwrap().clone());
    click(cx, "file-row-build.log");
    until(cx, "the end in view", |cx| {
        page.read_with(cx, |p, cx| {
            let code = p.code.read(cx);
            code.text().len() == log.len() && code.top_line() > 59_000 && code.at_end()
        })
    });
    assert!(cx.debug_bounds("files-edit").is_none());
    assert!(cx.debug_bounds("files-send").is_some());
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
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
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
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
        status(&shell, cx).contains("Permission denied")
    });
    menu_on(cx, "file-row-target");
    menu(cx, "Upload…");
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

/// The menu on a row copies its path and downloads it; the tree and the
/// file open are kept across a restart.
#[gpui::test]
fn file_menu_copies_and_downloads_and_the_tree_is_kept(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-menu-journey");
    let downloads = parvi_testkit::fresh_dir("files-menu-downloads");
    std::fs::create_dir(root.join("folder")).unwrap();
    let bytes = b"\x00\xffdownload\n";
    std::fs::write(root.join("folder/note.bin"), bytes).unwrap();
    std::fs::write(root.join("folder/read.txt"), "read").unwrap();
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
    let (shell, cx) = test_cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), Some(saved), w, cx)
    });
    tree(cx, "file-row-folder");
    click(cx, "file-row-folder");
    tree(cx, "file-row-note.bin");
    menu_on(cx, "file-row-note.bin");
    menu(cx, "Copy path");
    assert_eq!(
        cx.read_from_clipboard().and_then(|x| x.text()),
        Some(root.join("folder/note.bin").to_string_lossy().into_owned())
    );
    menu_on(cx, "file-row-note.bin");
    menu(cx, "Download");
    let destination = downloads.join("note.bin");
    let notice = format!("Downloaded to {}", destination.display());
    until(cx, "download and destination notice", |cx| {
        std::fs::read(&destination).ok().as_deref() == Some(bytes) && status(&shell, cx) == notice
    });
    click(cx, "file-row-read.txt");
    until(cx, "opened", |cx| {
        cx.debug_bounds("file-view-read.txt").is_some()
    });
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "tree saved", |_| {
        std::fs::read(&settings)
            .ok()
            .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok())
            .is_some_and(|v| v["files"]["file"].as_str() == root.join("folder/read.txt").to_str())
    });
    let url = running.url.clone();
    let saved = settings.clone();
    let (_, again) = test_cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), Some(saved), w, cx)
    });
    until(again, "the same tree and file after a restart", |cx| {
        cx.debug_bounds("file-view-read.txt").is_some()
            && cx.debug_bounds("file-row-note.bin").is_some()
            && cx.debug_bounds("picked-read.txt").is_some()
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
    tree(cx, "file-row-note.txt");
    click(cx, "file-row-note.txt");
    until(cx, "file viewer", |cx| {
        cx.debug_bounds("files-edit").is_some()
    });
    let viewer = cx.debug_bounds("files-viewer").unwrap();
    let page = cx.debug_bounds("files-page").unwrap();
    assert!(
        viewer.size.width >= page.size.width / 2.,
        "viewer should use at least half the window"
    );
    let row = cx.debug_bounds("file-row-note.txt").unwrap();
    let new = cx.debug_bounds("files-new").expect("New drawn");
    assert!(
        new.right() <= viewer.left() && new.bottom() <= row.top(),
        "New stays in the list's head"
    );
    for id in ["files-send", "files-edit"] {
        let action = cx.debug_bounds(id).expect("action drawn");
        assert!(
            action.right() <= viewer.right() && action.bottom() <= row.top(),
            "{id} stays in the viewer's head"
        );
    }
    menu_on(cx, "file-row-note.txt");
    assert!(
        cx.debug_bounds("file-view-note.txt").is_some(),
        "right-click keeps the viewer"
    );
    menu(cx, "Copy path");
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
    until(cx, "revealed file picked and open", |cx| {
        cx.debug_bounds("picked-note.txt").is_some()
            && cx.debug_bounds("file-view-note.txt").is_some()
            && cx.debug_bounds("files-edit").is_some()
    });
    cx.update(|_, cx| host.ask(reveal(&root), cx).unwrap());
    until(cx, "revealed root picked", |cx| {
        cx.debug_bounds("file-row-outside.txt").is_some()
            && cx.debug_bounds("file-row-clips.pdf").is_some()
            && cx.debug_bounds("picked-note.txt").is_none()
    });
    cx.update(|_, cx| host.ask(reveal(&folder), cx).unwrap());
    until(cx, "revealed folder picked, its contents shown", |cx| {
        cx.debug_bounds("picked-clips.pdf").is_some()
            && cx.debug_bounds("file-row-note.txt").is_some()
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
    let (_, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, window, cx)
    });
    tree(cx, "file-row-draft.txt");
    click(cx, "file-row-draft.txt");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("unsaved");
    until(cx, "says it is unsaved", |cx| {
        cx.debug_bounds("files-unsaved").is_some()
    });
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

/// A file's draft survives opening another file and folder and coming
/// back to it.
#[gpui::test]
fn opening_another_file_keeps_the_previous_files_draft(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-draft-navigation");
    std::fs::write(root.join("draft.txt"), "original").unwrap();
    std::fs::write(root.join("other.txt"), "other").unwrap();
    std::fs::create_dir(root.join("folder")).unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, window, cx)
    });
    tree(cx, "file-row-draft.txt");
    click(cx, "file-row-draft.txt");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    click(cx, "files-edit");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("kept draft");
    click(cx, "file-row-folder");
    until(cx, "the folder read", |cx| {
        cx.debug_bounds("files-note-Empty").is_some()
    });
    click(cx, "file-row-other.txt");
    until(cx, "the other file", |cx| {
        cx.debug_bounds("file-view-other.txt").is_some()
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
    let (_, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, window, cx)
    });
    for name in ["clip.mp4", "voice.mp3", "paper.pdf"] {
        let id = leak(format!("file-row-{name}"));
        tree(cx, id);
        std::fs::remove_file(root.join(name)).unwrap();
        click(cx, id);
        until(cx, "media open action", |cx| {
            cx.debug_bounds("files-play").is_some()
        });
    }
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// The list's keys pick rows, open and close folders, open files and copy
/// a path.
#[gpui::test]
fn the_list_takes_the_key_map(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-keys");
    std::fs::create_dir(root.join("b")).unwrap();
    std::fs::write(root.join("b/inner.txt"), "inner").unwrap();
    std::fs::write(root.join("a.txt"), "a").unwrap();
    std::fs::write(root.join("z.txt"), "z").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    tree(cx, "file-row-z.txt");
    cx.simulate_keystrokes("end ctrl-shift-c");
    let text = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(text.as_deref(), root.join("z.txt").to_str());
    cx.simulate_keystrokes("enter");
    until(cx, "viewer", |cx| {
        cx.debug_bounds("file-view-z.txt").is_some()
    });
    cx.simulate_keystrokes("up enter");
    until(cx, "the row above", |cx| {
        cx.debug_bounds("file-view-a.txt").is_some()
    });
    cx.simulate_keystrokes("up right");
    until(cx, "a folder opened by key", |cx| {
        cx.debug_bounds("file-row-inner.txt").is_some()
    });
    cx.simulate_keystrokes("left");
    until(cx, "and closed", |cx| {
        cx.debug_bounds("file-row-inner.txt").is_none()
    });
    cx.simulate_keystrokes("left left");
    until(cx, "the root closed", |cx| {
        cx.debug_bounds("file-row-z.txt").is_none()
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

/// Escape, or a click elsewhere, closes the menu without acting.
#[gpui::test]
fn escape_or_a_click_elsewhere_closes_the_menu(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-menu");
    std::fs::write(root.join("note.txt"), "text").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    tree(cx, "file-row-note.txt");
    menu_on(cx, "file-row-note.txt");
    cx.simulate_keystrokes("escape");
    until(cx, "menu dismissed", |cx| {
        cx.debug_bounds("menu-label-Copy path").is_none()
    });
    menu_on(cx, "file-row-note.txt");
    click(cx, "files-viewer");
    until(cx, "menu closed", |cx| {
        cx.debug_bounds("menu-label-Copy path").is_none()
    });
    assert!(root.join("note.txt").exists());
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn a_reload_keeps_the_tree_on_its_device(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-reload");
    let running = start(root.clone());
    let settings = root.join("settings.json");
    let saved = serde_json::json!({"files":{"tree":
        {"node":"away-node","open":["/example/models"],"selected":"/example/models/m.glb"}
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
    until(cx, "away device", |cx| {
        cx.debug_bounds("files-note-The device does not answer")
            .is_some()
    });
    let kept = kept(&shell, cx);
    assert_eq!(kept.tree.node, "away-node");
    assert_eq!(kept.tree.open, ["/example/models"]);
    assert_eq!(kept.tree.selected.as_deref(), Some("/example/models/m.glb"));
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
    let viewer = cx.debug_bounds("files-viewer").unwrap();
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

/// A find answers as typed, by names under every root; stale answers are
/// dropped; a find in the text goes to its line; more than fit says so.
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
    tree(cx, "file-row-sub");
    let page = SEARCH_PAGE.with(|page| page.borrow().as_ref().unwrap().clone());
    cx.simulate_keystrokes("/");
    cx.simulate_input("rea");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "first results", |cx| {
        cx.debug_bounds("hit-readme.md").is_some() && cx.debug_bounds("hit-realm.txt").is_some()
    });
    let old_revision = page.read_with(cx, |p, _| p.find.as_ref().unwrap().revision);
    cx.simulate_input("d");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "new results", |cx| {
        cx.debug_bounds("hit-readme.md").is_some() && cx.debug_bounds("hit-realm.txt").is_none()
    });
    page.update(cx, |p, cx| {
        p.find.as_mut().unwrap().answer(
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
    assert!(cx.debug_bounds("hit-readme.md").is_some());
    assert!(cx.debug_bounds("hit-stale").is_none());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("line marker");
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "a find in the text offered", |cx| {
        cx.debug_bounds("files-in-text").is_some()
    });
    click(cx, "files-in-text");
    until(cx, "content result", |cx| {
        cx.debug_bounds("hit-a.rs:12").is_some()
    });
    click(cx, "hit-a.rs:12");
    until(cx, "line 12 selected, lines above it in view", |cx| {
        page.read_with(cx, |p, cx| {
            let code = p.code.read(cx);
            code.selected_lines() == Some((12, 12)) && code.top_line() == 8
        })
    });
    click(cx, "files-search");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("capword");
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "truncation note", |cx| {
        cx.debug_bounds("files-note-Only the first 50; type more of it")
            .is_some()
    });
    click(cx, "files-search");
    cx.simulate_keystrokes("escape");
    until(cx, "the tree again", |cx| {
        cx.debug_bounds("file-row-sub").is_some()
            && page.read_with(cx, |p, cx| p.query.read(cx).text.is_empty())
    });
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn find_shows_a_hit_in_the_tree_and_its_text(cx: &mut TestAppContext) {
    let root = parvi_testkit::fresh_dir("files-find");
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(root.join("nested/needle.txt"), "found").unwrap();
    let running = start(root.clone());
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    tree(cx, "file-row-nested");
    cx.simulate_keystrokes("/");
    cx.simulate_input("needle");
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "search result", |cx| {
        cx.debug_bounds("hit-needle.txt").is_some()
    });
    menu_on(cx, "hit-needle.txt");
    menu(cx, "Copy path");
    assert_eq!(
        cx.read_from_clipboard().and_then(|v| v.text()),
        Some(
            root.join("nested/needle.txt")
                .to_string_lossy()
                .into_owned()
        )
    );
    menu_on(cx, "hit-needle.txt");
    menu(cx, "Show in tree");
    until(cx, "matched file in its folder", |cx| {
        cx.debug_bounds("picked-needle.txt").is_some()
    });
    click(cx, "file-row-needle.txt");
    until(cx, "viewer", |cx| cx.debug_bounds("files-edit").is_some());
    cx.simulate_keystrokes("/");
    cx.simulate_input("found");
    cx.executor().advance_clock(Duration::from_millis(300));
    until(cx, "a find in the text offered", |cx| {
        cx.debug_bounds("files-in-text").is_some()
    });
    cx.simulate_keystrokes("down enter");
    until(cx, "content result", |cx| {
        cx.debug_bounds("hit-needle.txt:1").is_some()
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
    let (_, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    tree(cx, "file-row-note.txt");
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
    let (_, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[parvi_files_app::build], Some(url), None, w, cx)
    });
    tree(cx, "file-row-note.txt");
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

/// Markdown reads rendered and JSON laid out, while editing either uses
/// the file's own text; binary offers a download; a refusal says why.
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
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(&[observed_files], Some(url), Some(settings), w, cx)
    });
    tree(cx, "file-row-README.md");
    let page = SEARCH_PAGE.with(|p| p.borrow().as_ref().unwrap().clone());
    click(cx, "file-row-README.md");
    until(cx, "rendered markdown", |cx| {
        cx.debug_bounds("files-markdown-rendered").is_some()
            && page.read_with(cx, |p, _| {
                p.opened
                    .as_ref()
                    .is_some_and(|o| matches!(o.show, Show::Text { markdown: Some(_) }))
            })
    });
    click(cx, "files-edit");
    until(cx, "its source to edit", |cx| {
        cx.debug_bounds("files-markdown-rendered").is_none()
            && cx.debug_bounds("files-code-top-1").is_some()
    });
    cx.simulate_keystrokes("escape");
    click(cx, "file-row-data.json");
    until(cx, "JSON laid out", |cx| {
        cx.debug_bounds("files-json-formatted").is_some()
    });
    assert_eq!(
        page.read_with(cx, |p, cx| p.pretty_code.read(cx).text().to_owned()),
        "{\n  \"a\": 1\n}"
    );
    click(cx, "files-edit");
    assert_eq!(
        page.read_with(cx, |p, cx| p.code.read(cx).text().to_owned()),
        "{\"a\":1}"
    );
    assert!(cx.debug_bounds("files-json-formatted").is_none());
    cx.simulate_keystrokes("escape");
    click(cx, "file-row-lines.txt");
    until(cx, "text view", |cx| {
        page.read_with(cx, |p, cx| {
            p.opened
                .as_ref()
                .is_some_and(|o| o.name == "lines.txt" && matches!(o.show, Show::Text { .. }))
                && p.code.read(cx).text() == "first\nsecond\n"
        })
    });
    click(cx, "file-row-data.bin");
    until(cx, "binary download", |cx| {
        cx.debug_bounds("viewer-download").is_some()
    });
    click(cx, "viewer-download");
    let destination = downloads.join("data.bin");
    let notice = format!("Downloaded to {}", destination.display());
    until(cx, "binary downloaded", |cx| {
        std::fs::read(&destination).ok().as_deref() == Some(&[0, 255, 32])
            && status(&shell, cx) == notice
    });
    click(cx, "file-row-secret.txt");
    until(cx, "permission denied", |cx| {
        cx.debug_bounds("files-failed").is_some()
            && page.read_with(cx, |p, _| p.opened.as_ref().is_some_and(|o| matches!(&o.show, Show::Failed(message) if message.contains("Permission denied"))))
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

/// Changes is offered only for a file its repository says changed; new
/// files, folders, a rename and a delete go through the menus.
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
    tree(cx, "file-row-tracked.txt");
    let page = SEARCH_PAGE.with(|p| p.borrow().as_ref().unwrap().clone());
    click(cx, "file-row-clean.txt");
    until(cx, "a clean file read", |cx| {
        cx.debug_bounds("files-edit").is_some()
            && page.read_with(cx, |p, _| {
                p.opened.as_ref().is_some_and(|o| o.name == "clean.txt")
            })
    });
    click(cx, "file-row-tracked.txt");
    until(cx, "Changes offered", |cx| {
        cx.debug_bounds("files-changes").is_some()
    });
    click(cx, "file-row-clean.txt");
    until(cx, "a clean file read again", |cx| {
        cx.debug_bounds("file-view-clean.txt").is_some() && cx.debug_bounds("files-edit").is_some()
    });
    assert!(
        cx.debug_bounds("files-changes").is_none(),
        "nothing to show"
    );
    click(cx, "file-row-tracked.txt");
    until(cx, "Changes offered", |cx| {
        cx.debug_bounds("files-changes").is_some()
    });
    click(cx, "files-changes");
    until(cx, "diff against HEAD", |cx| {
        cx.debug_bounds("files-diff").is_some()
            && page.read_with(cx, |p, _| {
                p.opened
                    .as_ref()
                    .and_then(|o| o.diff.as_ref())
                    .is_some_and(|d| d.diff.contains("-before\n+after"))
            })
    });
    menu_on(cx, "file-row-tracked.txt");
    menu(cx, "Rename");
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
    click(cx, "files-new");
    menu(cx, "New file");
    cx.simulate_input("new.txt");
    cx.simulate_keystrokes("enter");
    until(cx, "new file, open to write", |cx| {
        root.join("new.txt").is_file()
            && cx.debug_bounds("file-row-new.txt").is_some()
            && cx.debug_bounds("file-view-new.txt").is_some()
    });
    click(cx, "files-new");
    menu(cx, "New folder");
    cx.simulate_input("new-folder");
    click(cx, "files-name-apply");
    until(cx, "new folder", |cx| {
        root.join("new-folder").is_dir() && cx.debug_bounds("file-row-new-folder").is_some()
    });
    click(cx, "files-new");
    menu(cx, "New file");
    cx.simulate_input("abandoned.txt");
    cx.simulate_keystrokes("escape");
    until(cx, "naming cancelled", |cx| {
        cx.debug_bounds("files-name").is_none()
    });
    assert!(!root.join("abandoned.txt").exists());
    menu_on(cx, "file-row-new.txt");
    menu(cx, "Delete");
    assert!(root.join("new.txt").exists());
    menu(cx, "Delete");
    until(cx, "delete confirmed in the list and viewer", |cx| {
        !root.join("new.txt").exists()
            && cx.debug_bounds("file-row-new.txt").is_none()
            && cx.debug_bounds("file-view-new.txt").is_none()
            && page.read_with(cx, |p, _| p.opened.is_none())
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
    tree(cx, "file-row-pixel.png");
    click(cx, "file-row-pixel.png");
    until(cx, "zoom controls", |cx| {
        cx.debug_bounds("zoom-1").is_some()
    });
    click(cx, "zoom-1");
    assert_eq!(kept(&shell, cx).zoom, Some(100));
    click(cx, "zoom-fit");
    assert_eq!(kept(&shell, cx).zoom, None);
    drop(running);
    std::fs::remove_dir_all(root).unwrap();
}
