//! `parvi install` and `parvi upgrade`, the whole binary, against a stand-in
//! for `systemctl --user` on PATH: it runs a unit's ExecStart as a process of
//! its own and, as `KillMode=process` does, stops only that process. The
//! node serves a tmux server of the test's own. Nothing here reaches the real
//! systemd, units, install folders or tmux. Linux only; skips without tmux
//! unless `PARVI_REQUIRE_TMUX=1`.
#![cfg(target_os = "linux")]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use parvi::install::Unit;
use parvi_testkit::fresh_dir;
use parvi_testkit::tmux::{Server, eventually};
use serde_json::Value;

const PARVI: &str = env!("CARGO_BIN_EXE_parvi");
const UNIT: &str = "parvi-install-test";

/// Held by each test for its whole run. A test here writes a program and
/// then runs it; were another test to start a process in between, that
/// process would briefly hold the written file open and running it would
/// fail as "text file busy".
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `systemctl --user` for one scratch folder: calls go to `calls`, a
/// started unit's process id to `<unit>.pid`, its output to `<unit>.log`.
const FAKE_SYSTEMCTL: &str = r#"#!/bin/sh
echo "$*" >> "$FAKE_SYSTEMD/calls"
[ "$1" = --user ] || { echo "fake systemctl: only --user" >&2; exit 2; }
shift
command=$1
shift
while [ "${1#--}" != "$1" ]; do shift; done
service=$1
pidfile="$FAKE_SYSTEMD/$service.pid"
running() { [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; }
start() {
  line=$(sed -n 's/^ExecStart=//p' "$FAKE_SYSTEMD_UNITS/$service")
  eval "set -- $line"
  setsid "$@" >> "$FAKE_SYSTEMD/$service.log" 2>&1 < /dev/null &
  echo $! > "$pidfile"
}
stop() {
  if running; then
    pid=$(cat "$pidfile")
    kill "$pid"
    while kill -0 "$pid" 2>/dev/null; do sleep 0.05; done
  fi
  rm -f "$pidfile"
}
case $command in
  daemon-reload) ;;
  enable) touch "$FAKE_SYSTEMD/$service.enabled" ;;
  start) running || start ;;
  restart) stop; start ;;
  stop) stop ;;
  is-active) if running; then echo active; else echo inactive; exit 3; fi ;;
  *) echo "fake systemctl: no $command" >&2; exit 1 ;;
esac
"#;

/// One machine's worth of scratch: the fake systemd, the unit folder, the
/// install root, HOME and the config. Its node is stopped when it drops.
struct Machine {
    dir: PathBuf,
    port: u16,
}

impl Machine {
    fn new(tmux: &Server) -> Machine {
        let dir = fresh_dir("parvi-install");
        for sub in ["fakebin", "systemd", "units", "home", "xdg", "builds"] {
            std::fs::create_dir(dir.join(sub)).unwrap();
        }
        write_executable(&dir.join("fakebin/systemctl"), FAKE_SYSTEMCTL);
        std::fs::write(
            dir.join("config.toml"),
            format!(
                "[node]\ndevice = \"install-test\"\n[terminals]\nsocket = {:?}\n",
                tmux.socket_path()
            ),
        )
        .unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        Machine { dir, port }
    }

    /// A build of parvi at `builds/<name>/parvi`: a copy of this one.
    fn build(&self, name: &str) -> PathBuf {
        let dir = self.dir.join("builds").join(name);
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("parvi");
        std::fs::copy(PARVI, &path).unwrap();
        path
    }

    fn listen(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    fn installed(&self) -> PathBuf {
        self.dir.join("root/bin/parvi")
    }

    fn unit_file(&self) -> PathBuf {
        self.dir.join(format!("units/{UNIT}.service"))
    }

    /// Runs `parvi <verb> --json` from `build` with the scratch flags.
    fn parvi(&self, build: &Path, verb: &str, extra: &[&str]) -> Output {
        let mut command = Command::new(build);
        command.arg(verb).arg("--json");
        command.args(["--unit", UNIT, "--unit-dir"]);
        command.arg(self.dir.join("units"));
        command.args(["--timeout", "10"]);
        command.args(extra);
        let path = format!(
            "{}:{}",
            self.dir.join("fakebin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        command
            .env("PATH", path)
            .env("HOME", self.dir.join("home"))
            .env("XDG_CONFIG_HOME", self.dir.join("xdg"))
            .env("FAKE_SYSTEMD", self.dir.join("systemd"))
            .env("FAKE_SYSTEMD_UNITS", self.dir.join("units"));
        command.output().unwrap()
    }

    fn install(&self, build: &Path) -> Output {
        let root = self.dir.join("root");
        let config = self.dir.join("config.toml");
        self.parvi(
            build,
            "install",
            &[
                "--root",
                root.to_str().unwrap(),
                "--listen",
                &self.listen(),
                "--config",
                config.to_str().unwrap(),
            ],
        )
    }

    /// The node process the fake systemd runs now.
    fn node_pid(&self) -> Option<u32> {
        let text = std::fs::read_to_string(self.dir.join(format!("systemd/{UNIT}.service.pid")));
        text.ok()?.trim().parse().ok()
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("systemd/calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// `parvi <args> --node <this node>` with the installed CLI.
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(self.installed())
            .args(args)
            .args(["--node", &format!("http://{}", self.listen())])
            .output()
            .unwrap()
    }

    fn screen(&self, session: &str) -> String {
        let out = self.cli(&["session", "read", session]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        if let Some(pid) = self.node_pid() {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn write_executable(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn succeeded(out: &Output) -> Value {
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

async fn health(listen: &str) -> Value {
    let client = parvi_node_kit::NodeClient::new(Some(&format!("http://{listen}/"))).unwrap();
    let (health, _) = client
        .request(axum::http::Method::GET, "/api/health", None, None)
        .await
        .unwrap();
    health
}

/// Types `text` into `session` through the node, once the node lists it.
async fn send(machine: &Machine, session: &str, text: &str) {
    eventually(&format!("{text} sent to {session}"), || {
        machine
            .cli(&["session", "send", session, text])
            .status
            .success()
            .then_some(())
    })
    .await;
}

/// Waits until `session`'s screen through the node shows `line`.
async fn shows(machine: &Machine, session: &str, line: &str) {
    eventually(&format!("{session} shows {line}"), || {
        machine
            .screen(session)
            .lines()
            .any(|shown| shown.trim() == line)
            .then_some(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_keeps_every_session_and_the_new_node_serves_it() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let Some(tmux) = Server::start_in("parvi-upgrade") else {
        return;
    };
    tmux.new_session("alpha", 100, 30);
    let machine = Machine::new(&tmux);
    let first = machine.build("first");
    succeeded(&machine.install(&first));
    let pane = tmux.display("alpha", "#{pane_pid}");
    send(&machine, "alpha", "echo before-$((6*7))").await;
    shows(&machine, "alpha", "before-42").await;
    let old_node = machine.node_pid().unwrap();

    let second = machine.build("second");
    let upgraded = succeeded(&machine.parvi(&second, "upgrade", &[]));
    assert_eq!(upgraded["version"], parvi_node_kit::VERSION);
    assert_eq!(upgraded["previous"], parvi_node_kit::VERSION);

    let new_node = machine.node_pid().unwrap();
    assert_ne!(old_node, new_node, "the node did not restart");
    assert!(std::fs::read(machine.installed()).unwrap() == std::fs::read(&second).unwrap());
    // The same session, its same process and its screen, through the new node.
    assert_eq!(tmux.display("alpha", "#{pane_pid}"), pane);
    shows(&machine, "alpha", "before-42").await;
    send(&machine, "alpha", "echo after-$((6*7))").await;
    shows(&machine, "alpha", "after-42").await;
    assert_eq!(
        health(&machine.listen()).await["version"],
        parvi_node_kit::VERSION
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_that_fails_its_health_check_is_rolled_back() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let Some(tmux) = Server::start_in("parvi-rollback") else {
        return;
    };
    tmux.new_session("alpha", 100, 30);
    let machine = Machine::new(&tmux);
    let first = machine.build("first");
    succeeded(&machine.install(&first));
    send(&machine, "alpha", "echo kept-$((6*7))").await;
    shows(&machine, "alpha", "kept-42").await;

    // A build that says its version and runs upgrade, but cannot serve.
    let broken = machine.dir.join("builds/broken");
    std::fs::create_dir(&broken).unwrap();
    let broken = broken.join("parvi");
    write_executable(
        &broken,
        &format!(
            "#!/bin/sh\ncase \"$1\" in serve) echo 'broken build' >&2; exit 3 ;; esac\nexec {PARVI} \"$@\"\n"
        ),
    );
    let out = machine.parvi(
        &first,
        "upgrade",
        &["--from", broken.to_str().unwrap(), "--timeout", "3"],
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["code"], "upgrade_failed", "{error}");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(message.contains("no healthy node"), "{message}");
    assert!(message.contains("inactive"), "{message}");

    // The previous build is back, serving, with the session.
    assert!(std::fs::read(machine.installed()).unwrap() == std::fs::read(&first).unwrap());
    let health = health(&machine.listen()).await;
    assert_eq!(health["status"], "ok");
    shows(&machine, "alpha", "kept-42").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn install_is_idempotent_and_writes_the_expected_unit() {
    let _alone = ONE_AT_A_TIME.lock().await;
    let Some(tmux) = Server::start_in("parvi-install") else {
        return;
    };
    let machine = Machine::new(&tmux);
    let first = machine.build("first");
    let installed = succeeded(&machine.install(&first));
    assert_eq!(installed["changed"], true);
    assert_eq!(installed["listen"], machine.listen());
    let unit = std::fs::read_to_string(machine.unit_file()).unwrap();
    let mut environment = vec![(
        "PATH".to_owned(),
        format!(
            "{}:{}",
            machine.dir.join("fakebin").display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    )];
    if let Some(tmux) = std::env::var_os("PARVI_TMUX").filter(|v| !v.is_empty()) {
        environment.push(("PARVI_TMUX".into(), tmux.to_string_lossy().into()));
    }
    let expected = Unit {
        binary: machine.installed(),
        listen: machine.listen(),
        config: Some(machine.dir.join("config.toml")),
        environment,
    };
    assert_eq!(unit, expected.render());
    let service = format!("{UNIT}.service");
    assert_eq!(
        machine.calls(),
        [
            format!("--user is-active {service}"),
            "--user daemon-reload".into(),
            format!("--user enable --quiet {service}"),
            format!("--user start {service}"),
        ]
    );
    let pid = machine.node_pid().unwrap();

    let again = succeeded(&machine.install(&first));
    assert_eq!(again["changed"], false, "{again}");
    assert!(
        again["summary"]
            .as_str()
            .unwrap()
            .starts_with("Already installed")
    );
    assert_eq!(std::fs::read_to_string(machine.unit_file()).unwrap(), unit);
    assert_eq!(
        machine.node_pid(),
        Some(pid),
        "a second install restarted it"
    );
    assert_eq!(
        machine.calls()[4..],
        [
            format!("--user is-active {service}"),
            format!("--user enable --quiet {service}"),
        ]
    );

    // A different build is an upgrade's job, not install's.
    let other = machine.dir.join("builds/other");
    std::fs::create_dir(&other).unwrap();
    let other = other.join("parvi");
    std::fs::copy(PARVI, &other).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&other)
        .and_then(|mut file| std::io::Write::write_all(&mut file, b"\0"))
        .unwrap();
    let refused = machine.install(&other);
    assert_eq!(refused.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&refused.stderr).unwrap();
    assert_eq!(error["error"]["code"], "install_running", "{error}");
    assert_eq!(machine.node_pid(), Some(pid));
}
