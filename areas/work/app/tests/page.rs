//! The Work page's list, card and handoff against a node with a fixture store.
#![cfg(unix)]

use gpui::{App, Context, IntoElement, Render, div, prelude::*};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, Window};
use parvi_app_kit::ask::Ask;
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::{Host, Mounted, Page, mount};
use parvi_node_kit::{Built, Listing, Setup};
use parvi_terminals_app::Terminals;
use parvi_testkit::node::Node;
use parvi_testkit::tmux::Server;
use std::cell::RefCell;
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

static BD: OnceLock<String> = OnceLock::new();
static GH: OnceLock<String> = OnceLock::new();
fn build_work(setup: &Setup) -> anyhow::Result<Built> {
    parvi_work::build_with(
        setup,
        parvi_work::Bd::new(BD.get().unwrap()),
        parvi_work::Gh::new(GH.get().unwrap()),
    )
}
const WORK: Listing = Listing {
    build: build_work,
    ..parvi_work::LISTING
};
const AREAS: &[Listing] = &[parvi_terminals::LISTING, WORK];
thread_local! { static TERMINALS: RefCell<Option<Entity<Terminals>>> = const { RefCell::new(None) }; }
fn terminals(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
    mount(host, window, cx, |host, window, cx| {
        TERMINALS.with(|p| *p.borrow_mut() = Some(cx.entity()));
        Terminals::new(host, window, cx)
    })
}
struct Toy {
    host: Host,
}
impl Page for Toy {
    const ID: &'static str = "toy";
    const TITLE: &'static str = "Toy";
}
impl Render for Toy {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("toy-page")
            .debug_selector(|| "toy-page".into())
            .size_full()
            .flex()
            .child("Toy page")
            .children(self.host.beside(cx))
    }
}
fn toy(host: Host, window: &mut Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, |host, _, _| Toy { host })
}
struct Running {
    url: String,
    node: String,
    store: String,
    socket: std::path::PathBuf,
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
fn start() -> Option<Running> {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let Some(tmux) = Server::start_in("work-page-journey") else { ready.send(None).unwrap(); return; };
            tmux.new_session("alpha", 100, 30);
            let dir = parvi_testkit::fresh_dir("work-page-store");
            std::fs::create_dir_all(dir.join(".beads")).unwrap();
            let script = dir.join("fixture-bd");
            std::fs::write(&script, "#!/bin/sh\nif [ \"$2\" = config ]; then echo '{\"value\":\"fixture\"}'; exit 0; fi\nif [ -f slow ]; then sleep 6; fi\nif [ -f fail ]; then echo 'tracker unavailable' >&2; exit 1; fi\ncat fixture.json\n").unwrap();
            std::fs::write(dir.join("fixture.json"), r#"[
              {"id":"fixture-1","title":"Read the fixture","status":"open","created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:00Z","parent":"fixture-0","dependencies":[{"type":"blocks","depends_on_id":"fixture-3"}]},
              {"id":"fixture-0","title":"Parent title","status":"open","created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:00Z"},
              {"id":"fixture-2","title":"Child title","status":"open","created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:00Z","parent":"fixture-1"},
              {"id":"fixture-3","title":"Blocker title","status":"closed","created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:00Z"}
            ]"#).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            BD.set(script.to_string_lossy().into_owned()).unwrap();
            let gh = dir.join("fixture-gh");
            let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../node/tests/fixtures/work");
            std::fs::write(&gh, format!("#!/bin/sh\ncase \"$*\" in *number=51*) cat '{}';; *) cat '{}';; esac\n", base.join("item.json").display(), base.join("parvi.json").display())).unwrap();
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
            GH.set(gh.to_string_lossy().into_owned()).unwrap();
            let launch = dir.join("launch.conf");
            std::fs::write(&launch, format!("folder {}\n", dir.display())).unwrap();
            let config = format!("[terminals]\nsocket = {:?}\nlaunch = {:?}\n[work]\nprojects = [{:?}, {:?}]\n", tmux.socket_path(), launch, format!("beads {}", dir.display()), "github example-owner/parvi");
            let node = Node::start(AREAS, &config).await.unwrap();
            ready.send(Some((node.url(), node.id(), dir.display().to_string(), tmux.socket_path()))).unwrap();
            while stopped.try_recv().is_err() { tokio::time::sleep(Duration::from_millis(50)).await; }
            node.stop().await;
            std::fs::remove_dir_all(dir).unwrap();
        });
    });
    match wait.recv().unwrap() {
        Some((url, node, store, socket)) => Some(Running {
            url,
            node,
            store,
            socket,
            stop,
            thread: Some(thread),
        }),
        None => {
            thread.join().unwrap();
            assert!(
                std::env::var_os("PARVI_REQUIRE_TMUX").is_none(),
                "tmux is required"
            );
            None
        }
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
fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = cx
        .debug_bounds(Box::leak(selector.to_string().into_boxed_str()))
        .expect(selector)
        .center();
    cx.simulate_click(at, Modifiers::none());
}
fn shown(cx: &mut VisualTestContext) -> String {
    TERMINALS
        .with(|p| p.borrow().clone())
        .unwrap()
        .read_with(cx, |p, _| p.window_text(0))
        .unwrap_or_default()
}
#[gpui::test]
fn work_column_and_terminal_links_open_cards_and_hand_off(cx: &mut TestAppContext) {
    let Some(running) = start() else {
        return;
    };
    let settings = parvi_testkit::fresh_dir("work-page-settings");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let path = settings.join("app.json");
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[toy, terminals, parvi_work_app::build],
            Some(url),
            Some(path),
            window,
            cx,
        )
    });
    until(cx, "link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    let toy_host = shell.read_with(cx, |s, _| Host::new(s.hub()).for_page("toy", &[]));
    let reveal = |id: &str, beside| Ask::Reveal {
        what: "item".into(),
        id: id.into(),
        node: Some(parvi_core::NodeId(running.node.clone())),
        part: None,
        beside,
    };
    std::fs::write(std::path::Path::new(&running.store).join("slow"), "").unwrap();
    cx.update(|_, cx| toy_host.ask(reveal("fixture-1", true), cx))
        .unwrap();
    until(cx, "loading card", |cx| {
        cx.debug_bounds("work-beside-loading").is_some()
    });
    std::thread::sleep(Duration::from_millis(5300));
    cx.run_until_parked();
    assert!(cx.debug_bounds("work-beside-loading").is_some());
    assert!(cx.debug_bounds("work-beside-failed").is_none());
    std::fs::remove_file(std::path::Path::new(&running.store).join("slow")).unwrap();
    until(cx, "beside card title", |cx| {
        cx.debug_bounds("work-card-title-Read the fixture")
            .is_some()
    });
    // The family has its titles though the Work page never read the
    // store: the node answers them with the item.
    // A finished blocker folds into a line that unfolds it.
    for text in [
        "Part of-fixture-0",
        "Children-fixture-2",
        "Blocked by-finished",
    ] {
        assert!(
            cx.debug_bounds(Box::leak(format!("work-family-{text}").into_boxed_str()))
                .is_some(),
            "missing {text}"
        );
    }
    click(cx, "work-family-Blocked by-finished");
    until(cx, "the finished blocker", |cx| {
        cx.debug_bounds("work-family-Blocked by-fixture-3")
            .is_some()
    });
    click(cx, "work-family-Part of-fixture-0");
    until(cx, "parent card", |cx| {
        cx.debug_bounds("work-card-title-Parent title").is_some()
    });
    click(cx, "w-back");
    until(cx, "back to selected item", |cx| {
        cx.debug_bounds("work-card-title-Read the fixture")
            .is_some()
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    assert!(cx.debug_bounds("work-send").is_some());
    assert!(cx.debug_bounds("work-more").is_some());
    cx.simulate_keystrokes("alt-i");
    until(cx, "beside closed", |cx| {
        cx.debug_bounds("work-beside").is_none()
    });
    cx.update(|_, cx| toy_host.ask(reveal("fixture-404", true), cx))
        .unwrap();
    until(cx, "not found", |cx| {
        cx.debug_bounds(Box::leak(
            format!("work-beside-error-no Bead fixture-404 in {}", running.store).into_boxed_str(),
        ))
        .is_some()
    });
    assert!(cx.debug_bounds("work-beside-not-found").is_some());
    cx.simulate_keystrokes("alt-i");
    let failure = std::path::Path::new(&running.store).join("fail");
    std::fs::write(&failure, "").unwrap();
    // An item the page's board does not hold is asked of the node, whose
    // tracker fails now.
    cx.update(|_, cx| toy_host.ask(reveal("fixture-405", true), cx))
        .unwrap();
    until(cx, "tracker failure", |cx| {
        cx.debug_bounds("work-beside-error-tracker unavailable")
            .is_some()
    });
    assert!(cx.debug_bounds("work-beside-failed").is_some());
    assert!(cx.debug_bounds("work-beside-not-found").is_none());
    std::fs::remove_file(failure).unwrap();
    cx.simulate_keystrokes("alt-i");
    // The project is chosen by name on the page, and its tree lists.
    click(cx, "tab-work");
    until(cx, "Work page", |cx| {
        cx.debug_bounds("work-project").is_some()
    });
    cx.simulate_keystrokes("p");
    cx.simulate_input("fixture");
    until(cx, "the store to choose", |cx| {
        cx.debug_bounds("work-pick-fixture").is_some()
    });
    cx.simulate_keystrokes("enter");
    until(cx, "the store's tree", |cx| {
        cx.debug_bounds("work-row-fixture-2").is_some()
    });
    cx.update(|_, cx| toy_host.ask(reveal("fixture-1", false), cx))
        .unwrap();
    until(cx, "Work page card", |cx| {
        cx.debug_bounds("work-card-title-Read the fixture")
            .is_some()
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("work"));
    cx.update(|window, cx| shell.update(cx, |shell, cx| shell.show_id("terminals", window, cx)));
    let row = format!("row-{}::alpha", running.node);
    until(cx, "session", |cx| {
        cx.debug_bounds(Box::leak(row.clone().into_boxed_str()))
            .is_some()
    });
    click(cx, &row);
    until(cx, "peek", |cx| cx.debug_bounds("peek-attach").is_some());
    click(cx, "peek-attach");
    until(cx, "terminal", |cx| !shown(cx).trim().is_empty());
    // Both forms printed in the terminal resolve through the real Work page.
    for (written, title) in [
        ("fixture-1", "Read the fixture"),
        ("example-owner/parvi#51", "Example work item"),
    ] {
        let command = format!("printf '\\033[2J\\033[H{written}\\n'");
        let output = std::process::Command::new(
            std::env::var("PARVI_TMUX").unwrap_or_else(|_| "tmux".into()),
        )
        .args(["-f", "/dev/null", "-S"])
        .arg(&running.socket)
        .env_remove("TMUX")
        .args(["send-keys", "-t", "alpha", &command, "Enter"])
        .output()
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        until(cx, "reference printed in terminal", |cx| {
            shown(cx).lines().any(|line| line.trim() == written)
        });
        let point = TERMINALS
            .with(|p| p.borrow().clone())
            .unwrap()
            .read_with(cx, |p, _| {
                let text = p.window_text(0).unwrap();
                let row = text
                    .lines()
                    .position(|line| line.trim() == written)
                    .unwrap();
                let geo = p.geo.borrow()[0];
                let origin = geo.origin.unwrap();
                let cell = geo.cell.unwrap();
                gpui::point(
                    origin.x + cell.width * 3. + gpui::px(2.),
                    origin.y + cell.height * row as f32 + gpui::px(2.),
                )
            });
        cx.simulate_click(point, Modifiers::none());
        let selector = format!("work-card-title-{title}");
        until(cx, "linked item card", |cx| {
            cx.debug_bounds(Box::leak(selector.clone().into_boxed_str()))
                .is_some()
        });
        assert_eq!(
            shell.read_with(cx, |s, _| s.shown_page()),
            Some("terminals")
        );
        cx.simulate_keystrokes("alt-i");
        until(cx, "linked card closed", |cx| {
            cx.debug_bounds("work-beside").is_none()
        });
    }
    // Alt+B opens the Work panel over the terminals; it alone keeps the
    // Work channel on, and a row opens its item in the panel.
    cx.simulate_keystrokes("alt-b");
    until(cx, "fixture in the panel", |cx| {
        cx.debug_bounds("work-panel-row-fixture-1").is_some()
    });
    assert!(cx.debug_bounds("terminals-beside").is_some());
    click(cx, "work-panel-row-fixture-1");
    until(cx, "item in the panel", |cx| {
        cx.debug_bounds("work-card-title-Read the fixture")
            .is_some()
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.shown_page()),
        Some("terminals")
    );
    // Send in the panel hands the item to a session.
    click(cx, "work-send");
    until(cx, "send drawer", |cx| cx.debug_bounds("send-go").is_some());
    click(cx, "send-go");
    until(cx, "reference in session", |cx| {
        shown(cx).contains("bead fixture-1: Read the fixture")
    });
    until(cx, "send drawer closed", |cx| {
        cx.debug_bounds("send-go").is_none()
    });
    cx.simulate_keystrokes("alt-b");
    until(cx, "panel closed", |cx| {
        cx.debug_bounds("work-panel").is_none()
    });
    click(cx, "tab-work");
    until(cx, "item menu", |cx| cx.debug_bounds("work-more").is_some());
    let host = shell.read_with(cx, |shell, _| Host::new(shell.hub()));
    let sessions = cx
        .update(|_, cx| host.model::<parvi_terminals_contract::SessionList>(cx))
        .unwrap();
    // "…" offers to start a session in the item's checkout, first.
    click(cx, "work-more");
    until(cx, "menu", |cx| cx.debug_bounds("menu-row-0").is_some());
    click(cx, "menu-row-0");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let found = sessions.read_with(cx, |list, _| {
            list.sessions
                .iter()
                .any(|s| s.node == running.node && s.name == "fixture-1")
        });
        if found {
            break;
        }
        if Instant::now() >= deadline {
            let names = sessions.read_with(cx, |list, _| {
                list.sessions
                    .iter()
                    .map(|s| s.name.clone())
                    .collect::<Vec<_>>()
            });
            let status = shell.read_with(cx, |shell, cx| {
                shell
                    .hub()
                    .read(cx)
                    .status
                    .as_ref()
                    .map(|s| (s.text.clone(), s.failure))
            });
            panic!("item session did not start: sessions {names:?}, status {status:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    std::fs::remove_dir_all(settings).unwrap();
}
