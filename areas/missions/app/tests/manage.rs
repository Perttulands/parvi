#![cfg(unix)]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_node_kit::{Listing, NodeClient};
use parvi_testkit::{node::Node, sessions};
use serde_json::json;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
const AREAS: &[Listing] = &[
    sessions::LISTING,
    parvi_work::LISTING,
    parvi_missions::LISTING,
];
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
fn start() -> Running {
    let (ready, wait) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let node = Node::start(AREAS, "").await.unwrap();
                ready.send((node.url(), node.id())).unwrap();
                while stopped.try_recv().is_err() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                node.stop().await;
            })
    });
    let (url, node) = wait.recv_timeout(Duration::from_secs(20)).unwrap();
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
    mut f: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if f(cx) {
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn has(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .is_some()
}
fn click(cx: &mut VisualTestContext, selector: &str) {
    let bounds = cx
        .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .expect(selector);
    cx.simulate_click(bounds.center(), Modifiers::none());
}
#[gpui::test]
fn creates_and_renames_a_mission_with_cancel_and_conflict_guard(cx: &mut TestAppContext) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-manage-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    until(cx, "new mission", |cx| has(cx, "new-mission"));
    click(cx, "new-mission");
    until(cx, "create form", |cx| has(cx, "mission-form-name"));
    cx.simulate_input("Abandoned");
    click(cx, "mission-form-cancel");
    until(cx, "form canceled", |cx| !has(cx, "mission-form"));
    click(cx, "new-mission");
    until(cx, "fresh create form", |cx| has(cx, "mission-form-name"));
    cx.simulate_input("@@@");
    click(cx, "mission-form-submit");
    until(cx, "invalid title", |cx| has(cx, "mission-form-error"));
    click(cx, "mission-form-name");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("Release");
    click(cx, "mission-form-submit");
    until(cx, "created", |cx| !has(cx, "mission-form"));
    let rename = format!("mission-rename-{}-release", running.node);
    until(cx, "rename command", |cx| has(cx, &rename));
    click(cx, &rename);
    until(cx, "rename form", |cx| has(cx, "mission-form-name"));
    cx.simulate_keystrokes("ctrl-a backspace");
    click(cx, "mission-form-submit");
    until(cx, "name required", |cx| has(cx, "mission-form-error"));
    click(cx, "mission-form-name");
    cx.simulate_input("Renamed");
    click(cx, "mission-form-submit");
    until(cx, "renamed", |cx| !has(cx, "mission-form"));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let read = || {
        runtime.block_on(async {
            let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
            client
                .request(http::Method::GET, "/api/missions/release", None, None)
                .await
                .unwrap()
        })
    };
    assert_eq!(read().0["mission"]["title"], "Renamed");
    click(cx, &rename);
    until(cx, "rename again", |cx| has(cx, "mission-form-name"));
    cx.simulate_input("Draft name");
    runtime.block_on(async {
        let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
        let (doc, etag) = client
            .request(http::Method::GET, "/api/missions/release", None, None)
            .await
            .unwrap();
        client
            .request(
                http::Method::PATCH,
                "/api/missions/release",
                Some(json!({"title":"External", "expectedRev":doc["mission"]["rev"]})),
                Some(&etag),
            )
            .await
            .unwrap();
    });
    click(cx, "mission-form-submit");
    until(cx, "stale rename refused", |cx| {
        has(cx, "mission-form-error")
    });
    click(cx, "mission-form-name");
    cx.simulate_keystrokes("ctrl-a ctrl-c");
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("Draft name")
    );
    assert_eq!(read().0["mission"]["title"], "External");
    click(cx, "mission-form-cancel");
    until(cx, "stale form canceled", |cx| !has(cx, "mission-form"));
    click(cx, "new-mission");
    until(cx, "second create form", |cx| has(cx, "mission-form-name"));
    cx.simulate_input("Scratch");
    click(cx, "mission-form-submit");
    until(cx, "second mission created", |cx| !has(cx, "mission-form"));
    let delete = format!("mission-delete-{}-scratch", running.node);
    until(cx, "delete command", |cx| has(cx, &delete));
    click(cx, &delete);
    until(cx, "archive confirmation", |cx| {
        has(cx, "mission-archive-explanation")
    });
    click(cx, "mission-form-cancel");
    until(cx, "archive canceled", |cx| !has(cx, "mission-form"));
    click(cx, &delete);
    until(cx, "archive confirmation reopened", |cx| {
        has(cx, "mission-archive-explanation")
    });
    click(cx, "mission-form-submit");
    until(cx, "mission archived", |cx| {
        !has(cx, "mission-form") && !has(cx, &delete)
    });
    runtime.block_on(async {
        let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
        assert!(
            client
                .request(http::Method::GET, "/api/missions/scratch", None, None)
                .await
                .is_err()
        );
    });
}

#[gpui::test]
fn designs_a_multi_gate_mission_and_shows_findings_on_the_canvas(cx: &mut TestAppContext) {
    let running = start();
    let settings = std::env::temp_dir()
        .join(format!("parvi-design-{}", std::process::id()))
        .join("app.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = running.url.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(
            &[parvi_missions_app::build],
            Some(url),
            Some(settings),
            window,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(gpui::px(1600.), gpui::px(1000.)));
    until(cx, "new mission", |cx| has(cx, "new-mission"));
    click(cx, "new-mission");
    until(cx, "new mission form", |cx| has(cx, "mission-form-name"));
    cx.simulate_input("Design journey");
    click(cx, "mission-form-submit");
    let mission = format!("mission-{}-design-journey", running.node);
    until(cx, "mission listing", |cx| has(cx, &mission));
    click(cx, &mission);
    until(cx, "canvas control", |cx| has(cx, "mission-mode-canvas"));
    click(cx, "mission-mode-canvas");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = NodeClient::new(Some(&format!("{}/", running.url))).unwrap();
    let read = || {
        rt.block_on(client.request(
            http::Method::GET,
            "/api/missions/design-journey",
            None,
            None,
        ))
        .unwrap()
        .0["mission"]
            .clone()
    };
    until(cx, "board ready for edit", |cx| has(cx, "undo"));
    click(cx, "new-input");
    until(cx, "input saved", |_| {
        read()["inputCards"].as_array().unwrap().len() == 1
    });
    // The + beside the Input adds the next step, wired from it and open
    // for editing.
    let input = read()["inputCards"][0]["id"].as_str().unwrap().to_owned();
    let next = format!("canvas-next-{input}");
    until(cx, "next-step button", |cx| has(cx, &next));
    click(cx, &next);
    until(cx, "next step wired", |_| {
        let m = read();
        m["formations"].as_array().unwrap().len() == 1
            && m["connections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["from"].as_str().unwrap().starts_with(&input))
    });
    until(cx, "next step open", |cx| has(cx, "step-title"));
    for (button, field, count) in [("new-step", "formations", 2), ("new-gate", "gates", 1)] {
        until(cx, "board ready for edit", |cx| has(cx, "undo"));
        click(cx, button);
        until(cx, "card saved", |_| {
            read()[field].as_array().unwrap().len() == count
        });
    }
    let gate_id = read()["gates"][0]["id"].as_str().unwrap().to_owned();
    until(cx, "board ready to arrange", |cx| has(cx, "undo"));
    click(cx, "arrange");
    until(cx, "arranged layout saved", |_| {
        let layout = rt
            .block_on(client.request(
                http::Method::GET,
                "/api/missions/design-journey/layout",
                None,
                None,
            ))
            .unwrap()
            .0;
        layout["layout"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == gate_id && (node["x"] != 200 || node["y"] != 200))
    });
    // The findings panel narrows the canvas when it arrives; Fit after it.
    until(cx, "arranged canvas", |cx| {
        has(cx, "canvas-fit") && has(cx, "canvas-findings-list")
    });
    click(cx, "canvas-fit");
    let gate = format!("mission-card-{gate_id}");
    until(cx, "judge gate card", |cx| has(cx, &gate));
    click(cx, &gate);
    cx.run_until_parked();
    until(cx, "gate editor", |cx| has(cx, "step-extra"));
    click(cx, "step-extra");
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("formation");
    click(cx, "step-save");
    until(cx, "judge gate saved", |cx| !has(cx, "step-extra"));
    assert_eq!(read()["gates"][0]["kinds"][0], "formation");
    for (button, field, count) in [("new-gate", "gates", 2), ("new-end", "ends", 1)] {
        until(cx, "board ready for edit", |cx| has(cx, "undo"));
        click(cx, button);
        until(cx, "card saved", |_| {
            read()[field].as_array().unwrap().len() == count
        });
    }
    let doc = read();
    assert_eq!(doc["gates"][1]["kinds"][0], "human");
    let selector = format!(
        "validation-card-{}",
        doc["formations"][0]["id"].as_str().unwrap()
    );
    until(cx, "finding on a step card", |cx| has(cx, &selector));
}
