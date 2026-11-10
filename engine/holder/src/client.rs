//! Reaching a holder: find its pipe, start it if it isn't running, greet it,
//! and run commands or attach as a control client.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::windows::named_pipe::NamedPipeClient;

use crate::handshake::{self, Greeting};
use crate::reply::Reply;
use crate::sys::{detached, pipe};
use crate::words::quote;

/// How long a holder that was just started may take to open its pipe.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// Where a holder listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    path: String,
}

impl Endpoint {
    /// This user's holder.
    pub fn for_user() -> Result<Self> {
        Ok(Endpoint {
            path: pipe::path(&pipe::user_sid()?),
        })
    }

    /// A scratch holder of its own, for tests and experiments, so the
    /// operator's sessions stay untouched.
    pub fn scratch(name: &str) -> Self {
        Endpoint {
            path: pipe::path(name),
        }
    }

    /// The pipe's path, `\\.\pipe\parvi-holder-…`.
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// A greeted connection, before attaching.
pub struct Connection {
    reader: BufReader<ReadHalf<NamedPipeClient>>,
    writer: WriteHalf<NamedPipeClient>,
    greeting: Greeting,
}

/// A connection attached to a session: tmux control-mode lines come out of
/// `reader`, commands go into `writer`.
pub struct Attached {
    pub reader: BufReader<ReadHalf<NamedPipeClient>>,
    pub writer: WriteHalf<NamedPipeClient>,
}

/// Connects to the holder at `endpoint`, if one is running.
pub async fn connect(endpoint: &Endpoint) -> Result<Connection> {
    let pipe = pipe::open(endpoint.path())
        .await
        .with_context(|| format!("no holder on {}", endpoint.path()))?;
    let (read, mut writer) = tokio::io::split(pipe);
    writer.write_all(handshake::hello().as_bytes()).await?;
    writer.flush().await?;
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let greeting = handshake::greeting(&line)?;
    Ok(Connection {
        reader,
        writer,
        greeting,
    })
}

/// Connects to the holder at `endpoint`, first starting `exe` hidden and
/// detached if none is running. `log` overrides the holder's log file.
pub async fn connect_or_start(
    endpoint: &Endpoint,
    exe: &Path,
    log: Option<&Path>,
) -> Result<Connection> {
    match connect(endpoint).await {
        Ok(connection) => return Ok(connection),
        Err(error) if error.chain().any(|cause| cause.is::<std::io::Error>()) => {}
        Err(error) => return Err(error),
    }
    let mut line = format!("{} --pipe {}", arg(exe), arg(endpoint.path()));
    if let Some(log) = log {
        line.push_str(&format!(" --log {}", arg(log)));
    }
    // Sessions start where the holder runs unless told otherwise, and the
    // starter's directory may be a WSL path no shell can use.
    let home = std::env::var("USERPROFILE").ok();
    detached::start(&line, home.as_deref())?;
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        match connect(endpoint).await {
            Ok(connection) => return Ok(connection),
            Err(e) if tokio::time::Instant::now() > deadline => {
                return Err(e.context("the holder did not start"));
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}

/// One argument of a Windows command line, quoted.
fn arg(value: impl AsRef<std::ffi::OsStr>) -> String {
    format!("\"{}\"", value.as_ref().to_string_lossy())
}

impl Connection {
    /// What the holder said about itself.
    pub fn greeting(&self) -> &Greeting {
        &self.greeting
    }

    /// Runs one command, given as words. `Ok(Ok(output))` on success,
    /// `Ok(Err(message))` when the holder refused it.
    pub async fn command(&mut self, words: &[&str]) -> Result<Result<String, String>> {
        let line: Vec<String> = words.iter().map(|w| quote(w)).collect();
        self.writer
            .write_all(format!("{}\n", line.join(" ")).as_bytes())
            .await?;
        self.writer.flush().await?;
        self.reply().await
    }

    /// Streams a reply to `out` without collecting a pane's history in memory.
    /// This is used on a fresh connection, so no notifications are expected
    /// inside the reply and the command number identifies its closing marker.
    pub async fn stream_command<W: AsyncWrite + Unpin>(
        &mut self,
        words: &[&str],
        out: &mut W,
    ) -> Result<()> {
        let line: Vec<String> = words.iter().map(|w| quote(w)).collect();
        self.writer
            .write_all(format!("{}\n", line.join(" ")).as_bytes())
            .await?;
        self.writer.flush().await?;
        crate::reply::stream(&mut self.reader, out).await
    }

    /// Reads the next reply, skipping notifications.
    async fn reply(&mut self) -> Result<Result<String, String>> {
        let mut reply = Reply::default();
        let mut line = String::new();
        loop {
            line.clear();
            if self.reader.read_line(&mut line).await? == 0 {
                bail!("the holder closed the connection");
            }
            if let Some(done) = reply.line(line.trim_end_matches(['\r', '\n'])) {
                return Ok(done);
            }
        }
    }

    /// Attaches to `target` with tmux client `flags` (for the engine,
    /// `ignore-size,pause-after=1`). Lines after the attach reply belong to
    /// the caller.
    pub async fn attach(mut self, target: &str, flags: &str) -> Result<Attached> {
        let words = ["attach-session", "-f", flags, "-t", target];
        if let Err(message) = self.command(&words).await? {
            bail!("attach {target}: {message}");
        }
        Ok(Attached {
            reader: self.reader,
            writer: self.writer,
        })
    }
}
