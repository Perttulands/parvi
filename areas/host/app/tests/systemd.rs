//! A disposable user unit exercised through the app and CLI on a testkit node.
#![cfg(target_os = "linux")]
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use parvi_app_kit::link::client::LinkState;
use parvi_node_kit::{NodeClient, cli};
use parvi_testkit::node::Node;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

struct Unit {
    name: String,
    dir: PathBuf,
}
impl Unit {
    fn new() -> Self {
        let dir = parvi_testkit::fresh_dir("host-service-unit");
        let name = format!(
            "parvi-test-{}.service",
            dir.file_name().unwrap().to_string_lossy()
        );
        let file = dir.join(&name);
        std::fs::write(
            &file,
            "[Service]\nType=exec\nExecStart=/usr/bin/sleep 3600\n",
        )
        .unwrap();
        let out = Command::new("systemctl")
            .args(["--user", "link", "--runtime"])
            .arg(file)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Self { name, dir }
    }
}
impl Drop for Unit {
    fn drop(&mut self) {
        for args in [
            vec!["stop", self.name.as_str()],
            vec!["disable", "--runtime", self.name.as_str()],
            vec!["reset-failed", self.name.as_str()],
        ] {
            let _ = Command::new("systemctl").arg("--user").args(args).output();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
struct Running {
    url: String,
    id: String,
    config: PathBuf,
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
fn start(unit: &Unit) -> Running {
    let (ready, receive) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    let name = unit.name.clone();
    let thread =
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async move {
        let node = Node::start(&[parvi_host::LISTING], "[host]\nenabled = true\n").await.unwrap();
        let user = std::env::var("USER").unwrap();
        let component = json!({"kind":"systemd-user", "user":user, "unit":name});
        let missing = json!({"kind":"systemd-user", "user":user, "unit":format!("missing-{name}")});
        std::fs::write(node.dir().join("services.json"), json!({"services":[
            {"id":"single", "name":"Disposable", "components":[component.clone()]},
            {"id":"group", "name":"Disposable group", "components":[component.clone(), component]},
            {"id":"missing", "name":"Missing unit", "components":[missing]}
        ]}).to_string()).unwrap();
        ready.send((node.url(), node.id(), node.dir().join("config.toml"))).unwrap();
        while stopped.try_recv().is_err() { tokio::time::sleep(Duration::from_millis(20)).await; }
        node.stop().await;
    })
        });
    let (url, id, config) = receive.recv().unwrap();
    Running {
        url,
        id,
        config,
        stop,
        thread: Some(thread),
    }
}
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut test: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if test(cx) {
            return;
        }
        assert!(Instant::now() < end, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn selector(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}
fn click(cx: &mut VisualTestContext, id: &'static str) {
    let at = cx.debug_bounds(id).unwrap().center();
    cx.simulate_click(at, Modifiers::none());
}
fn command(url: &str, noun: &str, args: &[&str]) -> Value {
    let url = url.to_owned();
    let noun = noun.to_owned();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let mut args = args;
                args.extend(["--node".into(), url, "--json".into()]);
                let mut output = Vec::new();
                assert_eq!(cli::run(&noun, &args, &mut output).await.unwrap(), 0);
                serde_json::from_slice(&output).unwrap()
            })
    })
    .join()
    .unwrap()
}
#[gpui::test]
fn disposable_systemd_service_and_host_health_journey(cx: &mut TestAppContext) {
    let unit = Unit::new();
    let node = start(&unit);
    let temp = parvi_testkit::fresh_dir("host-service-app");
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let url = node.url.clone();
    let settings = temp.join("app.json");
    let (shell, cx) = cx.add_window_view(move |w, cx| {
        parvi_app_shell::start(
            &[parvi_host_app::build_host, parvi_host_app::build_services],
            Some(url),
            Some(settings),
            w,
            cx,
        )
    });
    until(cx, "node link", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub().read(cx).fleet.here().state == LinkState::Open
        })
    });
    click(cx, "tab-services");
    let state = |s| selector(format!("svc-state-{}-single-{s}", node.id));
    let action = |s| selector(format!("svc-{s}-{}-single", node.id));
    until(cx, "stopped unit and group", |cx| {
        cx.debug_bounds(state("stopped")).is_some()
            && cx
                .debug_bounds(selector(format!("svc-row-{}-group", node.id)))
                .is_some()
    });
    let listed = command(&node.url, "service", &["list"]);
    assert_eq!(listed["services"].as_array().unwrap().len(), 3);
    assert_eq!(
        listed["services"][1]["components"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    click(cx, action("start"));
    until(cx, "app start", |cx| {
        cx.debug_bounds(state("running")).is_some()
    });
    click(cx, action("restart"));
    until(cx, "app restart", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub()
                .read(cx)
                .toast
                .as_ref()
                .is_some_and(|v| v.text.contains("restarted"))
        })
    });
    click(cx, action("stop"));
    until(cx, "app stop", |cx| {
        cx.debug_bounds(state("stopped")).is_some()
    });
    for verb in ["start", "restart", "stop"] {
        let answer = command(&node.url, "service", &[verb, "single"]);
        assert_eq!(answer["outcome"], "done", "{answer}");
    }
    let missing = format!("missing-{}", unit.name);
    let expected = Command::new("systemctl")
        .args(["--user", "stop", &missing])
        .output()
        .unwrap();
    assert!(!expected.status.success());
    let words = String::from_utf8(expected.stderr)
        .unwrap()
        .trim()
        .to_string();
    let restart_missing = selector(format!("svc-restart-{}-missing", node.id));
    until(cx, "missing unit control", |cx| {
        cx.debug_bounds(restart_missing).is_some()
    });
    click(cx, restart_missing);
    until(cx, "systemd refusal", |cx| {
        shell.read_with(cx, |s, cx| {
            s.hub()
                .read(cx)
                .status
                .as_ref()
                .is_some_and(|status| status.text.contains(&words))
        })
    });
    let host = command(&node.url, "host", &["show"]);
    assert!(
        host["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "service")
    );
    let url = node.url.clone();
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let client = NodeClient::new(Some(&url)).unwrap();
                for path in ["/api/health", "/api/version"] {
                    let (body, _) = client
                        .request(axum::http::Method::GET, path, None, None)
                        .await
                        .unwrap();
                    assert!(body.is_object(), "{path}: {body}");
                }
            })
    })
    .join()
    .unwrap();
    click(cx, "tab-host");
    until(cx, "service action in Host events", |cx| {
        cx.debug_bounds("host-event-Disposable: stop done")
            .is_some()
    });
    let config = command(&node.url, "config", &["show"]);
    assert_eq!(config["sections"]["host"]["enabled"], true);
    assert_eq!(config["path"], node.config.to_string_lossy().as_ref());
    click(cx, "tab-settings");
    let config_row = selector("node-config:host:enabled = true".to_owned());
    until(cx, "host config on Settings", |cx| {
        cx.debug_bounds(config_row).is_some()
    });
    let settings = temp.join("app.json");
    let mut edited = shell.read_with(cx, |s, cx| s.hub().read(cx).settings.clone());
    edited.keys = false;
    std::fs::write(&settings, edited.text()).unwrap();
    until(cx, "external settings edit", |cx| {
        shell.read_with(cx, |s, cx| !s.hub().read(cx).settings.keys)
    });
    let toggle = "setting-app-keys";
    until(cx, "settings toggle", |cx| {
        cx.debug_bounds(toggle).is_some()
    });
    click(cx, toggle);
    until(cx, "toggle reflects external edit", |cx| {
        shell.read_with(cx, |s, cx| s.hub().read(cx).settings.keys)
    });
    let _ = std::fs::remove_dir_all(temp);
}
