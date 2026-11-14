//! Pages plug into the shell through `Page` and `Host` alone: two toy
//! pages, made here, each reach the node, the status line, menus, layers,
//! chords and each other through their Host (asks, a view beside the
//! asker, a read model, a lent view keeping its page's channel on), against a fake node
//! this test starts. A third,
//! whose area every node turned off, has no tab.

use futures_util::{SinkExt, StreamExt};
use gpui::{
    Entity, Modifiers, TestAppContext, VisualTestContext, WeakEntity, Window, div, point,
    prelude::*, px,
};
use parvi_app_kit::chords::Chord;
use parvi_app_kit::host::{Layer, Request};
use parvi_app_kit::link::client::LinkState;
use parvi_app_kit::link::socket::Tap;
use parvi_app_kit::menu::MenuRow;
use parvi_app_kit::{Answers, Ask, Build, Host, Mounted, Page, Setting, mount};
use parvi_app_shell::Shell;
use parvi_core::NodeId;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

const TOY: u8 = 0xc0;
/// Beta's kind.
const BETA_KIND: u8 = 0xc1;

/// A read model Alpha publishes: how many toy frames it heard.
#[derive(Default, PartialEq)]
struct Heard(usize);

thread_local! {
    static ALPHA: RefCell<Option<Entity<Alpha>>> = const { RefCell::new(None) };
    static BETA: RefCell<Option<Entity<Beta>>> = const { RefCell::new(None) };
}

struct Alpha {
    host: Host,
    me: WeakEntity<Alpha>,
    heard: Vec<String>,
    /// The kinds its tap was handed.
    tapped: Arc<Mutex<Vec<u8>>>,
    answer: Option<String>,
    revealed: Option<String>,
    chord: usize,
    row: usize,
    dismissed: bool,
    /// Its section as last read when a setting changed.
    settings: Option<serde_json::Value>,
    /// How many times it heard a setting change.
    changed: usize,
}

impl Alpha {
    fn build(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
        mount(host, window, cx, |host, _, cx| {
            ALPHA.with(|a| *a.borrow_mut() = Some(cx.entity()));
            let tapped = Arc::new(Mutex::new(vec![]));
            let seen = tapped.clone();
            let tap: Tap = Arc::new(move |_, kind, _| {
                seen.lock().expect("tapped").push(kind);
                false
            });
            host.tap(tap, cx);
            Alpha {
                host,
                me: cx.entity().downgrade(),
                heard: vec![],
                tapped,
                answer: None,
                revealed: None,
                chord: 0,
                row: 0,
                dismissed: false,
                settings: None,
                changed: 0,
            }
        })
    }
}

impl Page for Alpha {
    const ID: &'static str = "toy-a";
    const TITLE: &'static str = "Alpha";
    const KINDS: parvi_core::frame::Kinds = &[TOY..=TOY];
    const CHANNELS: &'static [u8] = &[TOY];
    const ANSWERS: &'static [Answers] = &[Answers::Reveal("toy")];
    const SETTINGS: &'static [Setting] = &[
        Setting::toggle("loud", "Loud", false),
        Setting::number("size", "Size", 1., (1., 3., 1.)),
        Setting::text("folder", "Folder", "").note("Folder for downloads."),
    ];

    fn settings_changed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.settings = Some(self.host.setting(cx).expect("its section reads"));
        self.changed += 1;
    }

    fn frame(&mut self, node: &NodeId, _: u8, body: &[u8], _: &mut Window, cx: &mut Context<Self>) {
        self.heard
            .push(format!("{node}:{}", String::from_utf8_lossy(body)));
        self.host
            .publish(Heard(self.heard.len()), cx)
            .expect("Alpha alone publishes Heard");
        cx.notify();
    }

    fn shown(&mut self, shown: bool, _: &mut Window, cx: &mut Context<Self>) {
        if !shown || self.answer.is_some() {
            return;
        }
        let asked = self
            .host
            .request(Request::get(NodeId::default(), "/toy"), cx);
        cx.spawn(async move |this, cx| {
            let got = asked.await;
            let _ = this.update(cx, |p, cx| {
                p.answer = Some(match got {
                    Ok(a) => format!("{} {}", a.status, String::from_utf8_lossy(&a.body)),
                    Err(e) => e.to_string(),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn answer(&mut self, ask: Ask, _: &'static str, _: &mut Window, cx: &mut Context<Self>) {
        if let Ask::Reveal { id, .. } = ask {
            self.revealed = Some(id);
        }
        cx.notify();
    }

    fn chords(&self, _: &gpui::App) -> Vec<Chord> {
        let me = self.me.clone();
        vec![
            Chord::new("t", "Toy chord", move |_, cx| {
                let _ = me.update(cx, |p, cx| {
                    p.chord += 1;
                    cx.notify();
                });
            })
            .alt("t", Some(false)),
        ]
    }
}

struct Note;

impl Render for Note {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child("A note")
    }
}

impl Render for Alpha {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (menu_host, layer_host) = (self.host.clone(), self.host.clone());
        let open_host = self.host.clone();
        let beside = self.host.beside(cx);
        let (me, me2) = (self.me.clone(), self.me.clone());
        div()
            .size_full()
            .flex()
            .gap(px(10.))
            .child(
                div()
                    .id("toy-menu")
                    .debug_selector(|| "toy-menu".into())
                    .w(px(80.))
                    .h(px(30.))
                    .child("Menu")
                    .on_click(move |_, _, cx| {
                        let me = me.clone();
                        let row = MenuRow::new("Toy row", move |_, cx| {
                            let _ = me.update(cx, |p, cx| {
                                p.row += 1;
                                cx.notify();
                            });
                        });
                        menu_host.menu(vec![row], point(px(300.), px(300.)), cx);
                    }),
            )
            .child(
                div()
                    .id("toy-layer")
                    .debug_selector(|| "toy-layer".into())
                    .w(px(80.))
                    .h(px(30.))
                    .child("Layer")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let me = me2.clone();
                        let layer =
                            Layer::new("note", cx.new(|_| Note)).on_dismiss(move |_, cx| {
                                let _ = me.update(cx, |p, cx| {
                                    p.dismissed = true;
                                    cx.notify();
                                });
                            });
                        layer_host.layer(layer, cx);
                    })),
            )
            .child(
                div()
                    .id("toy-open")
                    .debug_selector(|| "toy-open".into())
                    .w(px(80.))
                    .h(px(30.))
                    .child("Open beside")
                    .on_click(move |_, _, cx| {
                        let ask = Ask::OpenPath {
                            node: NodeId("toy-node".into()),
                            path: "/notes.txt".into(),
                            beside: true,
                        };
                        open_host.ask(ask, cx).expect("Beta answers it");
                    }),
            )
            .children(beside.map(|view| {
                div()
                    .debug_selector(|| "toy-beside".into())
                    .w(px(120.))
                    .h(px(30.))
                    .child(view)
            }))
    }
}

struct Beta {
    host: Host,
    refused: Option<String>,
    frames: Vec<String>,
    /// Alpha's read model, as Beta sees it.
    heard: Entity<Heard>,
    /// What it was asked to open, and by whom.
    opened: Option<(String, &'static str)>,
}

impl Beta {
    fn build(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
        mount(host, window, cx, |host, _, cx| {
            BETA.with(|b| *b.borrow_mut() = Some(cx.entity()));
            // Read before Alpha publishes: built in any order.
            let heard = host.model::<Heard>(cx).expect("the hub");
            cx.observe(&heard, |_, _, cx| cx.notify()).detach();
            Beta {
                host,
                refused: None,
                frames: vec![],
                heard,
                opened: None,
            }
        })
    }
}

impl Page for Beta {
    const ID: &'static str = "toy-b";
    const TITLE: &'static str = "Beta";
    const KINDS: parvi_core::frame::Kinds = &[BETA_KIND..=BETA_KIND];
    const ANSWERS: &'static [Answers] = &[Answers::OpenPath, Answers::Reveal("item")];

    fn frame(&mut self, _: &NodeId, _: u8, body: &[u8], _: &mut Window, cx: &mut Context<Self>) {
        self.frames.push(String::from_utf8_lossy(body).into());
        cx.notify();
    }

    /// Opens the path, or the item, beside the page that asked when it
    /// asks so, and on itself otherwise.
    fn answer(&mut self, ask: Ask, from: &'static str, _: &mut Window, cx: &mut Context<Self>) {
        let beside = ask.beside();
        let opened = match ask {
            Ask::OpenPath { path, .. } => path,
            Ask::Reveal { id, .. } => id,
            _ => return,
        };
        self.opened = Some((opened, from));
        if beside {
            self.host.show_beside(from, cx.new(|_| Note).into(), cx);
        }
    }
}

impl Render for Beta {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let host = self.host.clone();
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .id("toy-ask")
                    .debug_selector(|| "toy-ask".into())
                    .w(px(80.))
                    .h(px(30.))
                    .child("Ask")
                    .on_click(move |_, _, cx| {
                        let ask = Ask::Reveal {
                            what: "toy".into(),
                            id: "42".into(),
                            node: None,
                            part: None,
                            beside: false,
                        };
                        host.ask(ask, cx).expect("Alpha answers it");
                    }),
            )
            .child(
                div()
                    .id("toy-ask-none")
                    .debug_selector(|| "toy-ask-none".into())
                    .w(px(80.))
                    .h(px(30.))
                    .child("Ask nobody")
                    .on_click(cx.listener(|b, _, _, cx| {
                        let ask = Ask::Send {
                            reference: "x".into(),
                            target: None,
                        };
                        b.refused = b.host.ask(ask, cx).err().map(|e| e.code);
                        cx.notify();
                    })),
            )
    }
}

/// A page whose area every node has turned off.
struct Gamma;

impl Page for Gamma {
    const ID: &'static str = "toy-c";
    const TITLE: &'static str = "Gamma";
    const AREA: Option<&'static str> = Some("dark");
}

impl Render for Gamma {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn gamma(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
    mount(host, window, cx, |_, _, _| Gamma)
}

const PAGES: &[Build] = &[Alpha::build, Beta::build, gamma];

/// A node of one environment: it says hello with area `dark` off, answers
/// `GET /toy`, and once the toy channel is on sends one toy frame. What
/// the app sends it is kept in `heard`.
fn fake_node(heard: Arc<Mutex<Vec<Vec<u8>>>>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = listener.local_addr().expect("its address");
    listener.set_nonblocking(true).expect("nonblocking");
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(serve(stream, heard.clone()));
            }
        });
    });
    format!("http://{address}")
}

async fn serve(mut stream: tokio::net::TcpStream, heard: Arc<Mutex<Vec<Vec<u8>>>>) {
    let mut head = [0u8; 64];
    let Ok(n) = stream.peek(&mut head).await else {
        return;
    };
    if !head[..n].starts_with(b"GET /ws") {
        let mut request = vec![0u8; 4096];
        let _ = stream.read(&mut request).await;
        let (status, body) = if head[..n].starts_with(b"GET /toy ") {
            ("200 OK", r#"{"word":"pong"}"#)
        } else {
            ("404 Not Found", r#"{"error":"no such route"}"#)
        };
        let answer = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(answer.as_bytes()).await;
        return;
    }
    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let hello = serde_json::json!({
        "node": "toy-node", "device": "rig", "environment": "test",
        "version": "0.0.1", "protocol": 16, "off": ["dark"],
    });
    let mut frame = vec![0x00];
    frame.extend_from_slice(hello.to_string().as_bytes());
    if ws.send(Message::Binary(frame.into())).await.is_err() {
        return;
    }
    while let Some(Ok(msg)) = ws.next().await {
        let Message::Binary(bytes) = msg else {
            continue;
        };
        heard.lock().expect("heard").push(bytes.to_vec());
        if bytes[..] == [TOY, 1] {
            let toy = [&[TOY][..], b"ping"].concat();
            let beta = [&[BETA_KIND][..], b"pong"].concat();
            for frame in [toy, beta] {
                if ws.send(Message::Binary(frame.into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Runs the app until `ok`, or fails naming `what` after ten seconds.
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut ok: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if ok(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    cx.run_until_parked();
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
        .center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
}

fn settings_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("parvi-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch folder");
    dir.join("app.json")
}

fn status(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> String {
    shell.read_with(cx, |s, cx| {
        s.hub()
            .read(cx)
            .status
            .as_ref()
            .map(|s| s.text.clone())
            .unwrap_or_default()
    })
}

fn hello_device(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> String {
    shell.read_with(cx, |s, cx| {
        let hub = s.hub().read(cx);
        hub.fleet
            .here()
            .hello
            .as_ref()
            .map(|h| h.device.clone())
            .unwrap_or_default()
    })
}

fn alpha() -> Entity<Alpha> {
    ALPHA.with(|a| a.borrow().clone()).expect("Alpha was made")
}

#[gpui::test]
fn two_toy_pages_plug_in_through_their_host(cx: &mut TestAppContext) {
    let heard = Arc::new(Mutex::new(vec![]));
    let node = fake_node(heard.clone());
    let settings = settings_path("toy-pages");
    // The fake node and the link run on their own threads.
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let path = settings.clone();
    let address = node.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(PAGES, Some(node), Some(path), window, cx)
    });

    until(cx, "the link to open", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    // Connected as asked, the app says nothing; the node it reached is
    // the one the hello names.
    assert_eq!(status(&shell, cx), "");
    assert_eq!(hello_device(&shell, cx), "rig");

    // The tab bar: both toy pages, not the one whose area is off.
    cx.run_until_parked();
    assert!(cx.debug_bounds("tab-toy-a").is_some());
    assert!(cx.debug_bounds("tab-label-Alpha").is_some());
    assert!(cx.debug_bounds("tab-toy-b").is_some());
    assert!(cx.debug_bounds("tab-toy-c").is_none(), "its area is off");
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy-a"));

    // Shown, Alpha turned its channel on, heard its frame and asked over HTTP.
    let a = alpha();
    until(cx, "the toy frame and the HTTP answer", |cx| {
        a.read_with(cx, |a, _| !a.heard.is_empty() && a.answer.is_some())
    });
    a.read_with(cx, |a, _| {
        assert_eq!(a.heard, ["toy-node:ping"]);
        assert_eq!(a.answer.as_deref(), Some(r#"200 {"word":"pong"}"#));
    });
    assert!(heard.lock().expect("heard").contains(&vec![TOY, 1]));

    // Alpha's tap saw its own kind only: not the hello, not Beta's frame,
    // which reached Beta.
    let b = BETA.with(|b| b.borrow().clone()).expect("Beta was made");
    until(cx, "Beta's frame", |cx| {
        b.read_with(cx, |b, _| !b.frames.is_empty())
    });
    assert_eq!(
        *a.read_with(cx, |a, _| a.tapped.clone()).lock().unwrap(),
        [TOY]
    );

    // Beta reads what Alpha publishes; no other page may publish it.
    assert_eq!(b.read_with(cx, |b, cx| b.heard.read(cx).0), 1);
    let refused = shell.update(cx, |s, cx| {
        let other = Host::new(s.hub()).for_page("toy-b", &[]);
        other.publish(Heard(9), cx).unwrap_err()
    });
    assert_eq!(refused.code, "published_twice");
    assert_eq!(b.read_with(cx, |b, cx| b.heard.read(cx).0), 1);

    // Its chord, by Alt and by the leader; quietly, unless the operator
    // asked to see the keys.
    cx.simulate_keystrokes("alt-t");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("key-echo").is_none(),
        "no keys shown by default"
    );
    cx.simulate_keystrokes("ctrl-shift-space t");
    cx.run_until_parked();
    assert_eq!(a.read_with(cx, |a, _| a.chord), 2);
    shell.update(cx, |s, cx| {
        s.hub().update(cx, |h, _| h.settings.show_keys = true)
    });
    cx.simulate_keystrokes("alt-t");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("key-echo").is_some(),
        "the keys shown when asked"
    );
    assert_eq!(a.read_with(cx, |a, _| a.chord), 3);
    shell.update(cx, |s, cx| {
        s.hub().update(cx, |h, _| h.settings.show_keys = false)
    });

    // Its menu row.
    click(cx, "toy-menu");
    click(cx, "menu-row-0");
    assert_eq!(a.read_with(cx, |a, _| a.row), 1);

    // Its layer, on the one stack: Escape closes it and runs its dismiss.
    click(cx, "toy-layer");
    assert!(shell.read_with(cx, |s, cx| !s.hub().read(cx).stack.is_empty()));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(a.read_with(cx, |a, _| a.dismissed));
    assert!(shell.read_with(cx, |s, cx| s.hub().read(cx).stack.is_empty()));

    // Opened beside Alpha by Beta, Alpha staying shown.
    click(cx, "toy-open");
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy-a"));
    assert_eq!(
        b.read_with(cx, |b, _| b.opened.clone()),
        Some(("/notes.txt".into(), "toy-a"))
    );
    assert!(
        cx.debug_bounds("toy-beside").is_some(),
        "drawn beside Alpha"
    );

    // Beta, by its tab; Alpha's channel goes off as it hides.
    click(cx, "tab-toy-b");
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy-b"));
    until(cx, "the channel to go off", |_| {
        heard.lock().expect("heard").contains(&vec![TOY, 0])
    });

    // Beta reaches Alpha only by asking; what nobody answers says so.
    click(cx, "toy-ask-none");
    assert_eq!(
        b.read_with(cx, |b, _| b.refused.clone()).as_deref(),
        Some("unanswered")
    );
    assert_eq!(status(&shell, cx), "Nothing here can send to a session.");
    click(cx, "toy-ask");
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy-a"));
    assert_eq!(
        a.read_with(cx, |a, _| a.revealed.clone()).as_deref(),
        Some("42")
    );

    // The Settings page lists the shell's settings and what Alpha declares;
    // a change goes into Alpha's section, and Alpha hears it.
    click(cx, "tab-settings");
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("settings"));
    let node_field = cx
        .debug_bounds(Box::leak(
            format!("setting-app-node:{address}").into_boxed_str(),
        ))
        .unwrap();
    // Alpha's settings are its own group in the list.
    click(cx, "settings-group-toy-a");
    let folder_field = cx.debug_bounds("setting-toy-a-folder:").unwrap();
    assert!(node_field.size.width >= gpui::px(300.));
    assert_eq!(node_field.size.width, folder_field.size.width);
    click(cx, "setting-toy-a-loud");
    click(cx, "setting-toy-a-size-up");
    assert_eq!(
        a.read_with(cx, |a, _| a.settings.clone()),
        Some(serde_json::json!({"loud": true, "size": 2.0}))
    );
    click(cx, "settings-group-app");
    click(cx, "setting-app-keys");
    assert!(!shell.read_with(cx, |s, cx| s.hub().read(cx).settings.keys));

    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}

/// How many times the app turned the toy channel `on` (or off).
fn toy_channel(heard: &Mutex<Vec<Vec<u8>>>, on: bool) -> usize {
    let frame = vec![TOY, on as u8];
    heard
        .lock()
        .expect("heard")
        .iter()
        .filter(|f| **f == frame)
        .count()
}

#[gpui::test]
fn a_view_lent_beside_a_page_keeps_its_pages_channel_on(cx: &mut TestAppContext) {
    let heard = Arc::new(Mutex::new(vec![]));
    let node = fake_node(heard.clone());
    let settings = settings_path("lent");
    // The fake node and the link run on their own threads.
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let path = settings.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(PAGES, Some(node), Some(path), window, cx)
    });
    until(cx, "Alpha's channel on", |_| toy_channel(&heard, true) == 1);
    click(cx, "tab-toy-b");
    until(cx, "Alpha's channel off", |_| {
        toy_channel(&heard, false) == 1
    });

    // Alpha lends a view beside Beta while Beta shows: Alpha's channel
    // comes on and brings its frames.
    let a = alpha();
    let before = a.read_with(cx, |a, _| a.heard.len());
    let alpha_host = shell.read_with(cx, |s, _| Host::new(s.hub()).for_page("toy-a", &[]));
    cx.update(|_, cx| alpha_host.show_beside("toy-b", cx.new(|_| Note).into(), cx));
    until(cx, "the lent view's channel and frame", |cx| {
        toy_channel(&heard, true) == 2 && a.read_with(cx, |a, _| a.heard.len()) > before
    });
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy-b"));
    assert!(cx.update(|_, cx| alpha_host.beside_of("toy-b", cx).is_some()));

    // Across page switches it stays, and its channel is never turned off.
    click(cx, "tab-toy-a");
    click(cx, "tab-toy-b");
    assert!(cx.update(|_, cx| alpha_host.beside_of("toy-b", cx).is_some()));
    assert_eq!(toy_channel(&heard, false), 1);
    assert_eq!(toy_channel(&heard, true), 2);

    // Closed while Beta shows: gone, and the channel goes off.
    cx.update(|_, cx| alpha_host.close_beside("toy-b", cx));
    until(cx, "the channel off", |_| toy_channel(&heard, false) == 2);
    assert!(cx.update(|_, cx| alpha_host.beside_of("toy-b", cx).is_none()));
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}

#[gpui::test]
fn with_no_pages_the_shell_connects_to_its_node(cx: &mut TestAppContext) {
    let node = fake_node(Arc::new(Mutex::new(vec![])));
    let settings = settings_path("no-pages");
    // The fake node and the link run on their own threads.
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let path = settings.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(&[], Some(node), Some(path), window, cx)
    });
    until(cx, "the link to open", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    // Connected as asked, the app says nothing; the node it reached is
    // the one the hello names.
    assert_eq!(status(&shell, cx), "");
    assert_eq!(hello_device(&shell, cx), "rig");
    // Only the shell's own Settings page, from which the node is set.
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("settings"));
    assert!(cx.debug_bounds("tab-settings").is_some());
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}

/// Long enough for the app's settings write to come due.
const SAVED: Duration = Duration::from_millis(300);

/// The settings file as it is now.
fn on_disk(path: &std::path::Path) -> Option<parvi_app_kit::settings::Settings> {
    parvi_app_kit::settings::Settings::parse(&std::fs::read_to_string(path).ok()?)
}

#[gpui::test]
fn an_agents_edit_to_app_json_reaches_the_page_and_the_apps_own_writes_do_not(
    cx: &mut TestAppContext,
) {
    let node = fake_node(Arc::new(Mutex::new(vec![])));
    let settings = settings_path("reload");
    // The fake node, the link and the file watcher run on their own threads.
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let path = settings.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(PAGES, Some(node), Some(path), window, cx)
    });
    let hub = shell.read_with(cx, |s, _| s.hub().clone());
    let in_app = |cx: &mut VisualTestContext| hub.read_with(cx, |h, _| h.settings.clone());
    // Connected, the app keeps the fleet it saw: once its write is due, the
    // file holds what the app does.
    until(cx, "the link to open", |cx| {
        hub.read_with(cx, |h, _| h.fleet.here().state == LinkState::Open)
    });
    cx.executor().advance_clock(SAVED);
    until(cx, "the app's first write", |cx| {
        on_disk(&settings) == Some(in_app(cx))
    });
    let a = alpha();
    assert_eq!(a.read_with(cx, |a, _| a.changed), 0);

    // An agent turns Alpha's toggle on in the file: Alpha hears it at once.
    let mut edited = in_app(cx);
    edited.set_value("toy-a", "loud", serde_json::json!(true));
    let at = Instant::now();
    std::fs::write(&settings, edited.text()).expect("the agent's edit");
    until(cx, "Alpha to hear the edit", |cx| {
        a.read_with(cx, |a, _| a.changed == 1)
    });
    assert!(at.elapsed() < Duration::from_secs(2), "{:?}", at.elapsed());
    assert_eq!(
        a.read_with(cx, |a, _| a.settings.clone()),
        Some(serde_json::json!({"loud": true}))
    );
    assert!(status(&shell, cx).starts_with("Settings read again from "));

    // Turned off on the Settings page: one write, and Alpha hears it once.
    click(cx, "tab-settings");
    click(cx, "settings-group-toy-a");
    click(cx, "setting-toy-a-loud");
    assert_eq!(a.read_with(cx, |a, _| a.changed), 2);
    cx.executor().advance_clock(SAVED);
    until(cx, "the app's write", |_| {
        on_disk(&settings).and_then(|s| s.value("toy-a", "loud").cloned())
            == Some(serde_json::json!(false))
    });
    let written = std::fs::metadata(&settings)
        .and_then(|m| m.modified())
        .unwrap();
    // Time for the watcher to hear the app's own write, and for a write
    // that would follow it to come due.
    let settle = Instant::now() + Duration::from_millis(500);
    while Instant::now() < settle {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
    cx.executor().advance_clock(SAVED);
    cx.run_until_parked();
    assert_eq!(
        a.read_with(cx, |a, _| a.changed),
        2,
        "its own write is no edit"
    );
    let again = std::fs::metadata(&settings)
        .and_then(|m| m.modified())
        .unwrap();
    assert_eq!(written, again, "written once");

    // A file that does not read is reported; the last good settings stay.
    std::fs::write(&settings, "{not json").expect("a broken edit");
    until(cx, "the failure on the status line", |cx| {
        status(&shell, cx).contains("does not read")
    });
    assert!(shell.read_with(cx, |s, cx| {
        s.hub().read(cx).status.as_ref().unwrap().failure
    }));
    assert_eq!(
        in_app(cx).value("toy-a", "loud"),
        Some(&serde_json::json!(false))
    );
    assert_eq!(a.read_with(cx, |a, _| a.changed), 2);
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}

#[gpui::test]
fn a_reveal_marked_beside_is_answered_beside_the_asker(cx: &mut TestAppContext) {
    let node = fake_node(Arc::new(Mutex::new(vec![])));
    let settings = settings_path("reveal-beside");
    // The fake node and the link run on their own threads.
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let path = settings.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(PAGES, Some(node), Some(path), window, cx)
    });
    cx.run_until_parked();
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy-a"));
    let alpha_host = shell.read_with(cx, |s, _| Host::new(s.hub()).for_page("toy-a", &[]));
    let item = |beside| Ask::Reveal {
        what: "item".into(),
        id: "parvi-7".into(),
        node: None,
        part: None,
        beside,
    };
    let b = BETA.with(|b| b.borrow().clone()).expect("Beta was made");

    // Beside: Alpha stays shown and draws what Beta handed it.
    cx.update(|_, cx| alpha_host.ask(item(true), cx))
        .expect("Beta answers items");
    cx.run_until_parked();
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy-a"));
    assert_eq!(
        b.read_with(cx, |b, _| b.opened.clone()),
        Some(("parvi-7".into(), "toy-a"))
    );
    assert!(cx.update(|_, cx| alpha_host.beside(cx)).is_some());
    assert!(
        cx.debug_bounds("toy-beside").is_some(),
        "drawn beside Alpha"
    );

    // Not beside: Beta comes forward.
    cx.update(|_, cx| alpha_host.ask(item(false), cx))
        .expect("Beta answers items");
    cx.run_until_parked();
    assert_eq!(shell.read_with(cx, |s, _| s.shown_page()), Some("toy-b"));
    let _ = std::fs::remove_dir_all(settings.parent().expect("its folder"));
}
