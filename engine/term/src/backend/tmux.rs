//! The tmux backend: one tmux server, named by its socket.
//!
//! The control client is spoken in-process over tmux's socket
//! (`tmux_socket`), not run as a `tmux -C` process. Besides saving a process
//! per session, that keeps tmux alive: released tmux (3.4 to 3.6) crashes if
//! a client detaches while a control client is halfway through identifying,
//! and `tmux -C` identifies in pieces. Our handshake arrives in one piece.

use std::ffi::OsString;
use std::path::PathBuf;

use std::process::Stdio;

use anyhow::{Context, bail};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{Attaching, Backend, Reply, Stream};

/// Which tmux server to follow.
#[derive(Debug, Clone, Default)]
pub enum Socket {
    /// tmux's default socket.
    #[default]
    Default,
    /// `tmux -L <name>`.
    Name(String),
    /// `tmux -S <path>`.
    Path(PathBuf),
}

/// The tmux program: `PARVI_TMUX` when set, else `tmux` on `PATH`. Tests on a
/// host whose `tmux` is a guarding wrapper (CHROTE's refuses `kill-server`)
/// set it to the real binary.
pub fn program() -> OsString {
    std::env::var_os("PARVI_TMUX")
        .filter(|program| !program.is_empty())
        .unwrap_or_else(|| "tmux".into())
}

impl Socket {
    /// The server's socket file, resolved the way tmux does.
    #[cfg(unix)]
    pub fn path(&self) -> PathBuf {
        super::tmux_socket::socket_path(self)
    }

    pub(super) fn process(&self) -> Command {
        let mut command = Command::new(program());
        command.args(self.args()).env_remove("TMUX");
        command
    }

    async fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        let mut process = self.process();
        process
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = process.spawn().context("running tmux")?;
        let output = child.wait_with_output().await?;
        if !output.status.success() {
            bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn args(&self) -> Vec<OsString> {
        match self {
            Socket::Default => vec![],
            Socket::Name(name) => vec!["-L".into(), name.into()],
            Socket::Path(path) => vec!["-S".into(), path.into()],
        }
    }
}

impl Backend for Socket {
    #[cfg(unix)]
    fn launch_gate(&self, channel: &str) -> Option<String> {
        let program = std::path::PathBuf::from(program());
        let program = if program.is_absolute() {
            program
        } else {
            std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|dir| dir.join(&program))
                .find(|p| p.is_file())?
        };
        let program = std::fs::canonicalize(program).ok()?;
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let tmux = quote(program.to_str()?);
        let socket = quote(self.path().to_str()?);
        let channel = quote(channel);
        let script = format!(
            "shell=$({tmux} -S {socket} show-options -gv default-shell); timeout 10 {tmux} -S {socket} wait-for {channel}; exec \"$shell\" -l"
        );
        Some(format!("exec /bin/sh -c {}", quote(&script)))
    }

    fn create_session<'a>(&'a self, spec: &'a crate::SessionSpec) -> Reply<'a, crate::SessionRef> {
        Box::pin(async move {
            spec.validate()?;
            let cwd = spec
                .cwd
                .to_str()
                .context("session directory is not UTF-8")?;
            let cols = spec.cols.to_string();
            let rows = spec.rows.to_string();
            // tmux invokes a shell for one command argument. exec replaces that
            // shell, so the pane dies when the requested process does.
            let command = format!(
                "exec {}",
                spec.command
                    .iter()
                    .map(|s| format!("'{}'", s.replace('\'', "'\\''")))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
            let mut args = vec![
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{session_id} #{pane_id} #{pid} #{start_time}",
                "-s",
                &spec.name,
                "-c",
                cwd,
                "-x",
                &cols,
                "-y",
                &rows,
            ];
            for entry in &env {
                args.extend(["-e", entry]);
            }
            args.extend([
                &command,
                ";",
                "set-option",
                "-w",
                "-t",
                &spec.name,
                "window-size",
                "manual",
            ]);
            crate::owned::parse_ref(&self.run(&args).await?)
        })
    }

    fn kill_session<'a>(&'a self, session: &'a crate::SessionRef) -> Reply<'a, ()> {
        Box::pin(async move {
            let id = &session.session_id;
            if id
                .strip_prefix('$')
                .is_none_or(|n| n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()))
            {
                bail!("invalid session id");
            }
            let generation = session.generation;
            let condition = format!(
                "#{{&&:#{{==:#{{pid}},{}}},#{{==:#{{start_time}},{}}}}}",
                generation.pid, generation.start_time
            );
            let kill = format!("kill-session -t {id}");
            let reply = self
                .run(&[
                    "if-shell",
                    "-F",
                    &condition,
                    &kill,
                    "display-message -p generation-changed",
                ])
                .await?;
            if reply.trim() == "generation-changed" {
                bail!("server generation changed");
            }
            Ok(())
        })
    }

    fn command<'a>(&'a self, args: &'a [&'a str]) -> Reply<'a, String> {
        Box::pin(self.run(args))
    }

    fn size<'a>(&'a self, session: &'a str, cols: u16, rows: u16) -> Reply<'a, ()> {
        Box::pin(async move {
            self.run(&[
                "resize-window",
                "-t",
                session,
                "-x",
                &cols.to_string(),
                "-y",
                &rows.to_string(),
                ";",
                "set-window-option",
                "-t",
                session,
                "-u",
                "window-size",
            ])
            .await?;
            Ok(())
        })
    }

    fn stream<'a>(&'a self, args: &'a [&'a str]) -> Reply<'a, Stream> {
        Box::pin(async move {
            let mut child = self
                .process()
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("running tmux")?;
            let stdout = child.stdout.take().context("no tmux stdout")?;
            let mut stderr = child.stderr.take().context("no tmux stderr")?;
            // Read stderr while tmux runs, so a full pipe cannot stall it, and
            // report it like `run` does: a refusal names its reason.
            let completion = tokio::spawn(async move {
                let mut error = String::new();
                let (status, read) = tokio::join!(child.wait(), stderr.read_to_string(&mut error));
                read?;
                let status = status?;
                if !status.success() {
                    let error = error.trim();
                    if error.is_empty() {
                        bail!("tmux {status}");
                    }
                    bail!("{error}");
                }
                Ok(())
            });
            Ok(Stream {
                reader: Box::pin(tokio::io::BufReader::new(stdout)),
                completion,
            })
        })
    }

    /// Attaches with `ignore-size`, never sending a size, and with `-E`, so
    /// attaching doesn't overwrite the session's `update-environment`
    /// variables with ours.
    #[cfg(unix)]
    fn attach<'a>(&'a self, session: &'a str) -> Attaching<'a> {
        use super::{ControlClient, tmux_socket};
        Box::pin(async move {
            let path = self.path();
            let imsg = tmux_socket::imsg(self).await?;
            let attached = tmux_socket::attach(&path, imsg, session).await?;
            Ok(ControlClient {
                parser: crate::control::Parser::new(),
                reader: Box::pin(tokio::io::BufReader::new(attached.reader)),
                writer: Box::pin(attached.writer),
                watcher: Some(attached.watcher),
            })
        })
    }

    #[cfg(not(unix))]
    fn attach<'a>(&'a self, _session: &'a str) -> Attaching<'a> {
        let refused: anyhow::Result<super::ControlClient> =
            Err(anyhow::anyhow!("following tmux needs a unix system"));
        Box::pin(std::future::ready(refused))
    }
}
