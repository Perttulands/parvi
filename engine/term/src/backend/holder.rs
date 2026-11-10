//! A Windows holder reached through its per-user named pipe.

use std::path::PathBuf;

use anyhow::{Result, bail};
use parvi_holder::{Connection, Endpoint, connect_or_start};

use super::{Attaching, Backend, ControlClient, Reply, Stream};

#[derive(Clone)]
pub struct Holder {
    endpoint: Endpoint,
    executable: PathBuf,
    log: Option<PathBuf>,
}

impl Holder {
    /// The per-user holder, with its executable beside the node.
    pub fn for_user() -> Result<Self> {
        Self::new(Endpoint::for_user()?, None)
    }

    /// An isolated holder for tests.
    pub fn scratch(name: &str) -> Result<Self> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("holder name must contain only letters, digits, - and _");
        }
        Self::new(
            Endpoint::scratch(name),
            Some(std::env::temp_dir().join(format!("parvi-holder-{name}.log"))),
        )
    }

    fn new(endpoint: Endpoint, log: Option<PathBuf>) -> Result<Self> {
        let executable = std::env::current_exe()?.with_file_name("parvi-holder.exe");
        Ok(Self {
            endpoint,
            executable,
            log,
        })
    }

    async fn connect(&self) -> Result<Connection> {
        connect_or_start(&self.endpoint, &self.executable, self.log.as_deref()).await
    }
}

impl Backend for Holder {
    fn create_session<'a>(&'a self, spec: &'a crate::SessionSpec) -> Reply<'a, crate::SessionRef> {
        Box::pin(async move {
            spec.validate()?;
            let cwd = spec
                .cwd
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("session directory is not UTF-8"))?;
            let cols = spec.cols.to_string();
            let rows = spec.rows.to_string();
            let command = serde_json::to_string(&spec.command)?;
            let env = serde_json::to_string(&spec.env.iter().collect::<Vec<_>>())?;
            let mut connection = self.connect().await?;
            let reply = connection
                .command(&[
                    "parvi-create",
                    &spec.name,
                    cwd,
                    &cols,
                    &rows,
                    &command,
                    &env,
                ])
                .await?
                .map_err(anyhow::Error::msg)?;
            crate::owned::parse_ref(&reply)
        })
    }

    fn kill_session<'a>(&'a self, session: &'a crate::SessionRef) -> Reply<'a, ()> {
        Box::pin(async move {
            let mut connection = self.connect().await?;
            connection
                .command(&[
                    "parvi-kill",
                    &session.session_id,
                    &session.generation.pid.to_string(),
                    &session.generation.start_time.to_string(),
                ])
                .await?
                .map_err(anyhow::Error::msg)?;
            Ok(())
        })
    }

    fn command<'a>(&'a self, args: &'a [&'a str]) -> Reply<'a, String> {
        Box::pin(async move {
            let mut connection = self.connect().await?;
            connection.command(args).await?.map_err(anyhow::Error::msg)
        })
    }

    fn size<'a>(&'a self, session: &'a str, cols: u16, rows: u16) -> Reply<'a, ()> {
        Box::pin(async move {
            let mut connection = self.connect().await?;
            if let Err(message) = connection
                .command(&["attach-session", "-t", session])
                .await?
            {
                bail!("{message}");
            }
            if let Err(message) = connection
                .command(&["refresh-client", "-C", &format!("{cols},{rows}")])
                .await?
            {
                bail!("{message}");
            }
            Ok(())
        })
    }

    fn stream<'a>(&'a self, args: &'a [&'a str]) -> Reply<'a, Stream> {
        Box::pin(async move {
            let mut connection = self.connect().await?;
            let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
            let (read, mut write) = tokio::io::duplex(16 * 1024);
            let completion = tokio::spawn(async move {
                let words: Vec<&str> = args.iter().map(String::as_str).collect();
                connection.stream_command(&words, &mut write).await
            });
            Ok(Stream {
                reader: Box::pin(tokio::io::BufReader::new(read)),
                completion,
            })
        })
    }

    fn attach<'a>(&'a self, session: &'a str) -> Attaching<'a> {
        Box::pin(async move {
            let attached = self
                .connect()
                .await?
                .attach(session, "ignore-size,pause-after=1")
                .await?;
            Ok(ControlClient {
                parser: crate::control::Parser::holder(),
                reader: Box::pin(attached.reader),
                writer: Box::pin(attached.writer),
                watcher: None,
            })
        })
    }
}
