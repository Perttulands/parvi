//! The Agents page reads an isolated node and offers its tool details.
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::link::client::LinkState;
use parvi_testkit::{fresh_dir, node::Node, sessions};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Running {
    url: String,
    role_file: PathBuf,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
fn start() -> Running {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let home = fresh_dir("parvi-agents-page-home");
            let config = format!("[agents]\nhome = {:?}\npath = []\n", home);
            let node = Node::start(&[sessions::LISTING, parvi_agents::LISTING], &config)
                .await
                .unwrap();
            let role_file = node.dir().join("missions/roles/fixture-writer.toml");
            std::fs::create_dir_all(role_file.parent().unwrap()).unwrap();
            std::fs::write(&role_file, "schema = 1\n[card]\nid = \"fixture-writer\"\ndisplay_name = \"Fixture writer\"\nkind = \"specialist\"\nsummary = \"Before edit\"\nstatus = \"active\"\n").unwrap();
            ready.send((node.url(), role_file)).unwrap();
            while stopped.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            node.stop().await;
            std::fs::remove_dir_all(home).unwrap();
        });
    });
    let (url, role_file) = wait.recv().unwrap();
    Running {
        url,
        role_file,
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
#[gpui::test]
fn role_library_edits_and_saves_a_fixture_card(cx: &mut TestAppContext) {
    let running = start();
    let settings = fresh_dir("parvi-agents-page-settings");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let path = settings.join("app.json");
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_agents_app::build],
            Some(url),
            Some(path),
            window,
            cx,
        )
    });
    until(cx, "the node connection", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    until(cx, "the reported tool", |cx| {
        cx.debug_bounds("agents-tool-claude").is_some()
    });
    let at = cx.debug_bounds("agents-tool-claude").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    let at = cx.debug_bounds("agents-library").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    // The library opens on its first role, unasked.
    until(cx, "the first role's detail", |cx| {
        cx.debug_bounds("role-edit").is_some()
    });
    let at = cx.debug_bounds("role-fixture-writer").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    until(cx, "the fixture role's detail", |cx| {
        cx.debug_bounds("role-fixture-writer-shown").is_some()
    });
    until(cx, "the role detail", |cx| {
        cx.debug_bounds("role-edit").is_some()
    });
    let at = cx.debug_bounds("role-edit").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    until(cx, "the role form", |cx| {
        cx.debug_bounds("role-field-Summary").is_some()
    });
    let at = cx.debug_bounds("role-field-Summary").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("Updated from the library");
    let at = cx.debug_bounds("role-save").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    until(cx, "the edited card to be saved", |_| {
        std::fs::read_to_string(&running.role_file)
            .is_ok_and(|s| s.contains("summary = \"Updated from the library\""))
    });
    let saved = std::fs::read_to_string(&running.role_file).unwrap();
    assert!(saved.contains("id = \"fixture-writer\""));
    assert!(!saved.contains("Before edit"));
    until(cx, "the saved role detail", |cx| {
        cx.debug_bounds("role-edit").is_some()
    });
    std::fs::remove_dir_all(settings).unwrap();
}
