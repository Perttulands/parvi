//! Force output between a pause reseed's capture and cursor replies against
//! a real scratch tmux. The proxy gates a command; it does not fake screens.
#![cfg(unix)]

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::Result;
use parvi_term::backend::{Attaching, Backend, ControlClient, Reply, Stream, tmux::Socket};
use parvi_term::control::{Event, Notification, Parser};
use parvi_term::engine::{Engine, PaneUpdate};
use parvi_testkit::tmux::{Server, capture, normalise, write_script};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Notify, mpsc};

#[derive(Debug, PartialEq, Eq)]
enum Step {
    CursorHeld,
    Capture,
    LateOutput,
}

#[derive(Clone)]
struct Gated {
    socket: Socket,
    armed: Arc<AtomicBool>,
    steps: mpsc::UnboundedSender<Step>,
    release: Arc<Notify>,
}

impl Backend for Gated {
    fn create_session<'a>(
        &'a self,
        spec: &'a parvi_term::SessionSpec,
    ) -> Reply<'a, parvi_term::SessionRef> {
        self.socket.create_session(spec)
    }
    fn kill_session<'a>(&'a self, session: &'a parvi_term::SessionRef) -> Reply<'a, ()> {
        self.socket.kill_session(session)
    }
    fn command<'a>(&'a self, args: &'a [&'a str]) -> Reply<'a, String> {
        self.socket.command(args)
    }
    fn size<'a>(&'a self, session: &'a str, cols: u16, rows: u16) -> Reply<'a, ()> {
        self.socket.size(session, cols, rows)
    }
    fn stream<'a>(&'a self, args: &'a [&'a str]) -> Reply<'a, Stream> {
        self.socket.stream(args)
    }
    fn attach<'a>(&'a self, session: &'a str) -> Attaching<'a> {
        Box::pin(async move {
            let ControlClient {
                mut reader,
                mut writer,
                watcher,
                parser,
            } = self.socket.attach(session).await?;
            let (app_read, mut proxy_write) = tokio::io::duplex(64 * 1024);
            let (proxy_read, app_write) = tokio::io::duplex(64 * 1024);
            let armed = self.armed.clone();
            let steps = self.steps.clone();
            let release = self.release.clone();
            tokio::spawn(async move {
                let mut input = BufReader::new(proxy_read);
                let mut line = String::new();
                let mut capture_seen = false;
                let mut gated = false;
                loop {
                    line.clear();
                    if input.read_line(&mut line).await.unwrap() == 0 {
                        break;
                    }
                    if armed.load(Ordering::SeqCst)
                        && !gated
                        && line.starts_with("capture-pane -p -e -J ")
                    {
                        capture_seen = true;
                    }
                    if capture_seen && line.starts_with("display-message -p -t ") {
                        gated = true;
                        capture_seen = false;
                        let _ = steps.send(Step::CursorHeld);
                        release.notified().await;
                    }
                    if writer.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                    if writer.flush().await.is_err() {
                        break;
                    }
                }
                let _ = writer.shutdown().await;
            });
            let armed = self.armed.clone();
            let steps = self.steps.clone();
            tokio::spawn(async move {
                let mut parser = Parser::new();
                let mut line = Vec::new();
                let mut captured = false;
                let mut late = Vec::new();
                loop {
                    line.clear();
                    match reader.read_until(b'\n', &mut line).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let event = parser.line(line.strip_suffix(b"\n").unwrap_or(&line));
                    if proxy_write.write_all(&line).await.is_err() {
                        break;
                    }
                    if proxy_write.flush().await.is_err() {
                        break;
                    }
                    if !armed.load(Ordering::SeqCst) {
                        continue;
                    }
                    match event {
                        Some(Event::Reply {
                            from_us: true,
                            lines,
                            ..
                        }) if !captured && lines.iter().any(|s| s.contains("BEFORE")) => {
                            captured = true;
                            let _ = steps.send(Step::Capture);
                        }
                        Some(Event::Notification(Notification::Output { data, .. }))
                            if captured =>
                        {
                            late.extend_from_slice(&data);
                            if late.windows(5).any(|w| w == b"FINAL") {
                                let _ = steps.send(Step::LateOutput);
                                captured = false;
                            }
                        }
                        _ => {}
                    }
                }
            });
            Ok(ControlClient {
                reader: Box::pin(BufReader::new(app_read)),
                writer: Box::pin(app_write),
                watcher,
                parser,
            })
        })
    }
}

async fn next(steps: &mut mpsc::UnboundedReceiver<Step>) -> Step {
    tokio::time::timeout(Duration::from_secs(10), steps.recv())
        .await
        .expect("control step timed out")
        .expect("proxy closed")
}

#[tokio::test(flavor = "multi_thread")]
async fn late_output_after_pause_capture_is_not_lost() -> Result<()> {
    let Some(server) = Server::start_in("parvi-reseed-order") else {
        return Ok(());
    };
    let script = server.dir().join("producer");
    write_script(
        &script,
        "#!/bin/sh\nprintf '\\033[2J\\033[HBEFORE'\nIFS= read -r trigger\nprintf '\\033[2J\\033[HFINAL'\nIFS= read -r finish\n",
    );
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "race",
        "-x",
        "80",
        "-y",
        "24",
        script.to_str().unwrap(),
    ]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let armed = Arc::new(AtomicBool::new(false));
    let release = Arc::new(Notify::new());
    let engine = Engine::start(Gated {
        socket: server.socket(),
        armed: armed.clone(),
        steps: tx,
        release: release.clone(),
    });
    let mut changes = engine.watch();
    let pane = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(p) = engine
                .snapshot()
                .sessions
                .iter()
                .find(|s| s.name == "race")
                .and_then(|s| s.panes.first())
                .filter(|p| p.synced && p.text.contains("BEFORE"))
            {
                break p.id;
            }
            changes.changed().await.unwrap();
        }
    })
    .await
    .expect("initial screen");
    let mut viewer = engine.subscribe(pane).unwrap();
    assert!(viewer.screen.is_some());
    armed.store(true, Ordering::SeqCst);
    let client = server.run(&["list-clients", "-F", "#{client_name}"]);
    server.run(&[
        "refresh-client",
        "-t",
        client.trim(),
        "-A",
        &format!("%{pane}:pause"),
    ]);
    let (mut held, mut captured) = (false, false);
    while !held || !captured {
        match next(&mut rx).await {
            Step::CursorHeld => held = true,
            Step::Capture => captured = true,
            other => panic!("unexpected {other:?}"),
        }
    }
    server.run(&["send-keys", "-t", &format!("%{pane}"), "go", "Enter"]);
    assert_eq!(next(&mut rx).await, Step::LateOutput);
    let final_screen = normalise(&capture(&server, "race"));
    assert!(final_screen.contains("FINAL"), "{final_screen:?}");
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if matches!(
                viewer.updates.recv().await.unwrap(),
                PaneUpdate::Screen { .. }
            ) {
                break;
            }
        }
    })
    .await
    .expect("reseeded screen");
    let held = normalise(&engine.text(pane).expect("pane seeded"));
    assert_eq!(
        held, final_screen,
        "held screen must match tmux after the late output"
    );
    engine.stop().await;
    Ok(())
}
