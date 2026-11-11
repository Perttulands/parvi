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
fn build_work(setup: &Setup) -> anyhow::Result<Built> {
    parvi_work::build_with(
        setup,
        parvi_work::Bd::new(BD.get().unwrap()),
        parvi_work::Gh::default(),
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
            std::fs::write(&script, "#!/bin/sh\nif [ \"$2\" = config ]; then echo '{\"value\":\"fixture\"}'; exit 0; fi\nprintf '%s\\n' '[{\"id\":\"fixture-1\",\"title\":\"Read the fixture\",\"status\":\"open\",\"created_at\":\"2026-11-01T00:00:00Z\",\"updated_at\":\"2026-11-01T00:00:00Z\"}]'\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            BD.set(script.to_string_lossy().into_owned()).unwrap();
            let launch = dir.join("launch.conf");
            std::fs::write(&launch, format!("folder {}\n", dir.display())).unwrap();
            let config = format!("[terminals]\nsocket = {:?}\nlaunch = {:?}\n[work]\nprojects = [{:?}]\n", tmux.socket_path(), launch, format!("beads {}", dir.display()));
            let node = Node::start(AREAS, &config).await.unwrap();
            ready.send(Some((node.url(), node.id(), dir.display().to_string()))).unwrap();
            while stopped.try_recv().is_err() { tokio::time::sleep(Duration::from_millis(50)).await; }
            node.stop().await;
            std::fs::remove_dir_all(dir).unwrap();
        });
    });
    match wait.recv().unwrap() {
        Some((url, node, store)) => Some(Running {
            url,
            node,
            store,
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
fn opens_a_work_item_and_sends_its_reference_to_a_session(cx: &mut TestAppContext) {
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
    cx.update(|_, cx| toy_host.ask(reveal("fixture-1", true), cx))
        .unwrap();
    until(cx, "beside card title", |cx| {
        cx.debug_bounds("work-card-title-Read the fixture")
            .is_some()
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy"));
    assert!(cx.debug_bounds("work-send").is_some());
    assert!(cx.debug_bounds("work-start").is_some());
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
    cx.simulate_keystrokes("alt-i");
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
    // Alt+B docks Work while Terminals stays open; the dock alone watches
    // the Work channel and a row opens its card on the Work page.
    cx.simulate_keystrokes("alt-b");
    until(cx, "fixture in dock", |cx| {
        cx.debug_bounds("work-column-row-fixture-1").is_some()
    });
    assert!(cx.debug_bounds("dock-work").is_some());
    click(cx, "work-column-row-fixture-1");
    until(cx, "item card", |cx| cx.debug_bounds("work-card").is_some());
    cx.simulate_keystrokes("alt-b");
    until(cx, "dock closed", |cx| {
        cx.debug_bounds("dock-work").is_none()
    });
    until(cx, "send action", |cx| {
        cx.debug_bounds("work-send").is_some()
    });
    click(cx, "work-send");
    until(cx, "send drawer", |cx| cx.debug_bounds("send-go").is_some());
    click(cx, "send-go");
    until(cx, "reference in session", |cx| {
        shown(cx).contains("bead fixture-1: Read the fixture")
    });
    until(cx, "send drawer closed", |cx| {
        cx.debug_bounds("send-go").is_none()
    });
    click(cx, "tab-work");
    until(cx, "start action", |cx| {
        cx.debug_bounds("work-start").is_some()
    });
    let host = shell.read_with(cx, |shell, _| Host::new(shell.hub()));
    let sessions = cx
        .update(|_, cx| host.model::<parvi_terminals_contract::SessionList>(cx))
        .unwrap();
    click(cx, "work-start");
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
