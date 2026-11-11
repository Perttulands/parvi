#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::{Ask, Host};
use parvi_core::NodeId;
use parvi_node_kit::{Listing, NodeClient};
use parvi_testkit::{node::Node, sessions};
use serde_json::json;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const AREAS: &[Listing] = &[
    sessions::LISTING,
    parvi_work::LISTING,
    parvi_missions::LISTING,
];
struct Running {
    url: String,
    id: String,
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
fn start() -> Running {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let node = Node::start(AREAS, "").await.unwrap();
                let folder = node.dir().join("missions/missions");
                std::fs::create_dir_all(&folder).unwrap();
                std::fs::write(
                    folder.join("hello.mission.toml"),
                    include_str!("fixtures/solo-lab.mission.toml"),
                )
                .unwrap();
                ready.send((node.url(), node.id())).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                node.stop().await;
            });
    });
    let (url, id) = wait.recv().unwrap();
    Running {
        url,
        id,
        stop,
        thread: Some(thread),
    }
}
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut f: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        if f(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[gpui::test]
fn saving_merges_untouched_concurrent_fields_and_refuses_a_same_field_change(
    cx: &mut TestAppContext,
) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-editor-merge-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(900.)));
    shell.update(cx, |shell, cx| {
        Host::new(shell.hub())
            .for_page("test", &[])
            .ask(
                Ask::Reveal {
                    what: "mission".into(),
                    id: "hello".into(),
                    node: Some(NodeId(running.id.clone())),
                    part: None,
                    beside: false,
                },
                cx,
            )
            .unwrap()
    });
    until(cx, "mission view", |cx| {
        cx.debug_bounds("mission-mode-canvas").is_some()
            || cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    if let Some(bounds) = cx.debug_bounds("mission-mode-canvas") {
        cx.simulate_click(bounds.center(), Modifiers::none());
    }
    until(cx, "step", |cx| {
        cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    let point = cx.debug_bounds("mission-card-fmn_work").unwrap().center();
    cx.simulate_click(point, Modifiers::none());
    until(cx, "editor", |cx| cx.debug_bounds("step-goal").is_some());
    let point = cx.debug_bounds("step-delete").unwrap().center();
    cx.simulate_click(point, Modifiers::none());
    until(cx, "delete confirmation", |cx| {
        cx.debug_bounds("delete-keep").is_some()
    });
    let point = cx.debug_bounds("delete-keep").unwrap().center();
    cx.simulate_click(point, Modifiers::none());
    until(cx, "delete cancelled", |cx| {
        cx.debug_bounds("step-delete").is_some()
    });
    assert!(cx.debug_bounds("step-goal").is_some());
    let point = cx.debug_bounds("step-goal").unwrap().center();
    cx.simulate_click(point, Modifiers::none());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("My goal");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
    rt.block_on(async {let(doc,etag)=client.request(http::Method::GET,"/api/missions/hello",None,None).await.unwrap();client.request(http::Method::PATCH,"/api/missions/hello",Some(json!({"expectedRev":doc["mission"]["rev"],"updateFormation":{"id":"fmn_work","title":"Renamed elsewhere"}})),Some(&etag)).await.unwrap();});
    let point = cx.debug_bounds("step-save").unwrap().center();
    cx.simulate_click(point, Modifiers::none());
    until(cx, "merged save", |cx| {
        cx.debug_bounds("step-goal").is_none()
    });
    rt.block_on(async {
        let (doc, _) = client
            .request(http::Method::GET, "/api/missions/hello", None, None)
            .await
            .unwrap();
        assert_eq!(
            doc["mission"]["formations"][0]["title"],
            "Renamed elsewhere"
        );
        assert_eq!(doc["mission"]["formations"][0]["brief"]["goal"], "My goal");
    });
    until(cx, "step again", |cx| {
        cx.debug_bounds("mission-card-fmn_work").is_some()
    });
    let point = cx.debug_bounds("mission-card-fmn_work").unwrap().center();
    cx.simulate_click(point, Modifiers::none());
    until(cx, "editor reopened", |cx| {
        cx.debug_bounds("step-goal").is_some()
    });
    let point = cx.debug_bounds("step-goal").unwrap().center();
    cx.simulate_click(point, Modifiers::none());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("My second goal");
    rt.block_on(async {let(doc,etag)=client.request(http::Method::GET,"/api/missions/hello",None,None).await.unwrap();let f=&doc["mission"]["formations"][0];let mut brief=f["brief"].clone();brief["goal"]=json!("Other goal");client.request(http::Method::PATCH,"/api/missions/hello",Some(json!({"expectedRev":doc["mission"]["rev"],"editFormation":{"id":"fmn_work","title":f["title"],"brief":brief}})),Some(&etag)).await.unwrap();});
    let point = cx.debug_bounds("step-save").unwrap().center();
    cx.simulate_click(point, Modifiers::none());
    until(cx, "same field conflict", |cx| {
        cx.debug_bounds("step-error-The brief was changed elsewhere while this window was open. Close it and open the card again to see the change.").is_some()
    });
    let point = cx.debug_bounds("step-goal").unwrap().center();
    cx.simulate_click(point, Modifiers::none());
    cx.simulate_keystrokes("ctrl-a ctrl-c");
    assert_eq!(
        cx.read_from_clipboard().and_then(|v| v.text()).as_deref(),
        Some("My second goal")
    );
}
