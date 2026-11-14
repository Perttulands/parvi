//! A Brain reference travels through the real Send drawer into a scratch tmux session.
#![cfg(unix)]
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, Window};
use parvi_app_kit::{Host, Mounted, link::client::LinkState, mount};
use parvi_terminals_app::Terminals;
use parvi_testkit::{fresh_dir, node::Node, tmux::Server};
use std::{
    cell::RefCell,
    sync::mpsc,
    time::{Duration, Instant},
};

thread_local! { static TERMINALS: RefCell<Option<Entity<Terminals>>> = const { RefCell::new(None) }; }
fn terminals(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
    mount(host, window, cx, |host, window, cx| {
        TERMINALS.with(|p| *p.borrow_mut() = Some(cx.entity()));
        Terminals::new(host, window, cx)
    })
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
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
fn start() -> Option<Running> {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let Some(tmux) = Server::start_in("brain-send-journey") else {
                    ready.send(None).unwrap();
                    return;
                };
                tmux.new_session("alpha", 100, 30);
                let corpus = fresh_dir("brain-send-corpus");
                std::fs::create_dir(corpus.join("notes")).unwrap();
                std::fs::write(corpus.join("notes/page.md"), "# Page\n").unwrap();
                // A page arrives by a commit.
                for args in [
                    &["init", "-q"][..],
                    &["config", "user.name", "Fixture"],
                    &["config", "user.email", "fixture@example.org"],
                    &["add", "."],
                    &["commit", "-qm", "Fixture"],
                ] {
                    let done = std::process::Command::new("git")
                        .arg("-C")
                        .arg(&corpus)
                        .args(args)
                        .output()
                        .unwrap();
                    assert!(
                        done.status.success(),
                        "{}",
                        String::from_utf8_lossy(&done.stderr)
                    );
                }
                let config = format!(
                    "[brain]\ncorpus = {:?}\n[terminals]\nsocket = {:?}\n",
                    corpus,
                    tmux.socket_path()
                );
                let node = Node::start(&[parvi_terminals::LISTING, parvi_brain::LISTING], &config)
                    .await
                    .unwrap();
                ready.send(Some((node.url(), node.id()))).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                node.stop().await;
                std::fs::remove_dir_all(corpus).unwrap();
            });
    });
    match wait.recv().unwrap() {
        Some((url, node)) => Some(Running {
            url,
            node,
            stop,
            thread: Some(thread),
        }),
        None => {
            thread.join().unwrap();
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
    let selector = Box::leak(selector.to_string().into_boxed_str());
    let at = cx.debug_bounds(selector).expect(selector).center();
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
fn brain_send_reaches_a_real_session(cx: &mut TestAppContext) {
    let Some(running) = start() else {
        return;
    };
    let settings = fresh_dir("brain-send-settings");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (shell, cx) = cx.add_window_view(|window, cx| {
        parvi_app_shell::start(
            &[terminals, parvi_brain_app::build],
            Some(running.url.clone()),
            Some(settings.join("app.json")),
            window,
            cx,
        )
    });
    until(cx, "node link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    let row = format!("row-{}::alpha", running.node);
    let selector = Box::leak(row.clone().into_boxed_str());
    until(cx, "session", |cx| cx.debug_bounds(selector).is_some());
    click(cx, &row);
    until(cx, "attach", |cx| cx.debug_bounds("peek-attach").is_some());
    click(cx, "peek-attach");
    until(cx, "session screen", |cx| !shown(cx).trim().is_empty());
    click(cx, "tab-brain");
    until(cx, "the page among the arrivals", |cx| {
        cx.debug_bounds("brain-arrived-notes/page.md").is_some()
    });
    click(cx, "brain-arrived-notes/page.md");
    until(cx, "reader", |cx| {
        cx.debug_bounds("brain-reading-notes/page.md").is_some()
    });
    click(cx, "brain-send");
    until(cx, "send drawer", |cx| cx.debug_bounds("send-go").is_some());
    click(cx, "send-go");
    until(cx, "reference in session", |cx| {
        let text = shown(cx);
        text.contains("brain page /") && text.contains("/notes/page.md")
    });
    std::fs::remove_dir_all(settings).unwrap();
}
