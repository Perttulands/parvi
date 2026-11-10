//! A real tmux server on a scratch socket, for tests: only ever the test's
//! own, stopped by its own process id.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use parvi_term::backend::tmux::{Socket, program};

use crate::fresh_dir;
use parvi_term::engine::{Engine, Snapshot};

/// The start of the name of every private directory `Server::start` makes.
pub const SERVER_DIR: &str = "parvi-tmux";

/// A tmux server on a socket in a private directory this test created, so
/// the test can only ever reach its own server. Runs `PARVI_TMUX` (default
/// `tmux`). When the test ends its server is stopped by its process id and,
/// once that process has exited, the directory is removed. A server that
/// cannot be confirmed stopped fails the test and keeps its directory.
pub struct Server {
    /// Unique to this server; tests name their own scratch files after it.
    pub name: String,
    dir: PathBuf,
}

impl Server {
    /// Starts a server with no config file. `None` means tmux isn't installed
    /// and the test should skip.
    pub fn start() -> Option<Server> {
        Server::start_in(SERVER_DIR)
    }

    /// Like `start`, in a private directory whose name begins with `label`,
    /// which may hold any words: a test can put tmux's diagnostics in its path.
    pub fn start_in(label: &str) -> Option<Server> {
        if Command::new(program()).arg("-V").output().is_err() {
            assert!(
                std::env::var_os("PARVI_REQUIRE_TMUX").is_none(),
                "tmux is required but not installed"
            );
            eprintln!("skipping: tmux is not installed");
            return None;
        }
        let dir = fresh_dir(label);
        // Shells in this server find these before any real harness, so a
        // test that types a bare `claude` or `codex` fails loudly instead of
        // starting the operator's agent (#215). Tests run fakes by path.
        let stubs = dir.join("no-harness");
        std::fs::create_dir(&stubs).unwrap();
        for harness in HARNESSES {
            write_script(
                &stubs.join(harness),
                "#!/bin/sh\necho \"parvi test: $0 is a stub; run a fake harness by its path\" >&2\nexit 97\n",
            );
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        Some(Server { name, dir })
    }

    /// The private directory holding this server's socket. Removed, with
    /// everything in it, once the server has stopped.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The socket the server listens on.
    pub fn socket_path(&self) -> PathBuf {
        self.dir.join("tmux")
    }

    pub fn socket(&self) -> Socket {
        Socket::Path(self.socket_path())
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(program());
        command
            .arg("-f")
            .arg("/dev/null")
            .arg("-S")
            .arg(self.socket_path())
            .env_remove("TMUX")
            .env("PATH", stubbed_path(&self.dir.join("no-harness")));
        command
    }

    /// Runs a tmux command and returns its stdout. Panics on failure.
    pub fn run(&self, args: &[&str]) -> String {
        let output = self.command().args(args).output().expect("run tmux");
        assert!(
            output.status.success(),
            "tmux {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A detached session running a plain shell with a fixed prompt.
    pub fn new_session(&self, name: &str, cols: u16, rows: u16) {
        self.run(&[
            "new-session",
            "-d",
            "-s",
            name,
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
            "env PS1='$ ' sh",
        ]);
    }

    pub fn window_size(&self, target: &str) -> String {
        self.display(target, "#{window_width}x#{window_height}")
    }

    pub fn display(&self, target: &str, format: &str) -> String {
        self.run(&["display-message", "-p", "-t", target, format])
            .trim()
            .to_owned()
    }

    /// Everything about a session that a careless client could change.
    pub fn fingerprint(&self, session: &str) -> String {
        format!(
            "size={} window-options=[{}] session-options=[{}]",
            self.window_size(session),
            self.run(&["show-options", "-w", "-t", session]).trim(),
            self.run(&["show-options", "-t", session]).trim(),
        )
    }

    pub fn clients(&self) -> Vec<String> {
        self.run(&[
            "list-clients",
            "-F",
            "#{client_session} control=#{client_control_mode} #{client_width}x#{client_height} #{client_flags}",
        ])
        .lines()
        .map(str::to_owned)
        .collect()
    }

    /// A viewer that sizes the window, like a browser tile or an SSH client:
    /// a control client that has told tmux its size.
    pub fn viewer(&self, session: &str, cols: u16, rows: u16) -> Viewer {
        self.viewer_with(session, cols, rows, &[])
    }

    /// A viewer that only watches while another client sizes the window, like
    /// a CHROTE 2 tile that found the sizing seat taken (ADR-0017).
    pub fn observer(&self, session: &str, cols: u16, rows: u16) -> Viewer {
        self.viewer_with(session, cols, rows, &["-f", "ignore-size"])
    }

    pub fn viewer_with(&self, session: &str, cols: u16, rows: u16, flags: &[&str]) -> Viewer {
        let mut child = self
            .command()
            .args(["-C", "attach-session"])
            .args(flags)
            .args(["-t", session])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("start viewer");
        writeln!(
            child.stdin.as_mut().unwrap(),
            "refresh-client -C {cols},{rows}"
        )
        .unwrap();
        Viewer { child }
    }
}

impl Server {
    /// Ends the server as `kill-server` does, returning only once its
    /// process has exited: `kill-server` answers before the server is gone,
    /// so a command sent right after it can reach the exiting server. The
    /// next command that needs a server starts a new one on the same socket.
    pub fn kill(&self) {
        if let Err(error) = self.stop() {
            panic!("tmux server in {}: {error}", self.dir.display());
        }
    }

    /// Stops this test's server, if one runs, and waits until its process has
    /// exited. The socket is in this test's private directory, so whatever
    /// answers there is this test's; it is stopped by its process id.
    fn stop(&self) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let pid = loop {
            let output = self
                .command()
                .args(["display-message", "-p", "#{pid}"])
                .output()
                .map_err(|e| format!("running tmux: {e}"))?;
            let error = String::from_utf8_lossy(&output.stderr);
            if output.status.success() {
                let pid = String::from_utf8_lossy(&output.stdout);
                break pid
                    .trim()
                    .parse::<libc::pid_t>()
                    .map_err(|e| format!("its pid {pid:?}: {e}"))?;
            }
            if absent(&error) {
                return Ok(());
            }
            // A server exiting with its last session: ask again.
            if !exiting(&error) || Instant::now() > deadline {
                return Err(format!("cannot reach it to stop it: {error}"));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        // SAFETY: kill(2) only sends a signal; the pid is this test's server.
        if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(format!("stopping server {pid}: {error}"));
            }
        }
        // The server exits after the signal is delivered, not before kill returns.
        loop {
            // SAFETY: signal 0 only asks whether the process exists.
            if unsafe { libc::kill(pid, 0) } != 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(format!("server {pid} still runs after SIGTERM"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let stopped = self.stop().and_then(|()| {
            std::fs::remove_dir_all(&self.dir).map_err(|e| format!("removing it: {e}"))
        });
        if let Err(error) = stopped {
            let error = format!("tmux server in {}: {error}", self.dir.display());
            // A second panic while unwinding would abort the test run.
            if std::thread::panicking() {
                eprintln!("{error}");
            } else {
                panic!("{error}");
            }
        }
    }
}

/// Whether tmux found no server at the socket: no socket file, or a stale one
/// that refuses connections. Only the words around the path are read.
fn absent(error: &str) -> bool {
    let error = error.trim();
    error.starts_with("no server running on ")
        || (error.starts_with("error connecting to ")
            && error.ends_with(" (No such file or directory)"))
}

/// Whether tmux reached a server that closed the connection as it exited.
fn exiting(error: &str) -> bool {
    error.trim().trim_start_matches('[').trim_end_matches(']') == "server exited unexpectedly"
}

/// The harnesses a test must never start for real.
pub const HARNESSES: [&str; 2] = ["claude", "codex"];

/// `PATH` with `stubs` first.
pub fn stubbed_path(stubs: &Path) -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    format!("{}:{path}", stubs.display())
}

/// Writes an executable script without this process ever holding it
/// open for writing. A file written here and run at once fails with
/// ETXTBSY whenever another test's `Command` forks while the write
/// handle is open: the forked child holds a copy until it execs, and
/// Linux refuses to run a file open for writing anywhere. Here the only
/// write handle is `cat`'s, in a child that has exited before the run.
pub fn write_script(path: &Path, text: impl AsRef<str>) {
    let mut cat = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    cat.stdin
        .take()
        .unwrap()
        .write_all(text.as_ref().as_bytes())
        .unwrap();
    assert!(cat.wait().unwrap().success(), "writing {}", path.display());
}

pub struct Viewer {
    child: Child,
}

impl Viewer {
    pub fn resize(&mut self, cols: u16, rows: u16) {
        writeln!(
            self.child.stdin.as_mut().unwrap(),
            "refresh-client -C {cols},{rows}"
        )
        .unwrap();
    }

    pub fn detach(mut self) {
        drop(self.child.stdin.take());
        self.child.wait().unwrap();
    }
}

/// Waits until `session`'s window is `size` (`COLSxROWS`).
pub async fn window_becomes(server: &Server, session: &str, size: &str) {
    eventually(&format!("the window at {size}"), || {
        (server.window_size(session) == size).then_some(())
    })
    .await;
}

/// Polls `check` until it returns `Some`, or panics after ten seconds.
pub async fn eventually<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub fn all_synced(snapshot: &Snapshot, sessions: &[&str]) -> bool {
    sessions.iter().all(|name| {
        snapshot
            .sessions
            .iter()
            .any(|s| s.name == *name && !s.panes.is_empty() && s.panes.iter().all(|p| p.synced))
    })
}

/// Trailing blanks don't matter: tmux and the emulator disagree on them.
pub fn normalise(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let end = lines
        .iter()
        .rposition(|line| !line.is_empty())
        .map_or(0, |i| i + 1);
    lines[..end].join("\n")
}

pub fn pane_text(engine: &Engine, session: &str) -> Option<String> {
    let snapshot = engine.snapshot();
    let session = snapshot.sessions.iter().find(|s| s.name == session)?;
    let pane = session.panes.first().filter(|p| p.synced)?;
    Some(pane.text.clone())
}

/// The first pane of the session named `session` as the node's sessions
/// read it, one line per row, once its screen is read.
pub fn session_text(sessions: &dyn parvi_term::Sessions, session: &str) -> Option<String> {
    let listed = sessions.list();
    let (pane, _, _) = *listed.iter().find(|s| s.name == session)?.panes.first()?;
    let screen = sessions.screen(pane)?;
    let rows: Vec<String> = (screen.rows.iter())
        .map(|row| {
            let line: String = (row.iter())
                .map(|cell| match (cell.text.as_str(), cell.width) {
                    ("", 1) => " ",
                    (text, _) => text,
                })
                .collect();
            line.trim_end().to_owned()
        })
        .collect();
    Some(rows.join("\n"))
}

pub fn capture(server: &Server, session: &str) -> String {
    server.run(&["capture-pane", "-p", "-t", session])
}

/// The user running the tests, who owns every scratch tmux server.
pub fn whoami() -> String {
    let out = Command::new("id").arg("-un").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
