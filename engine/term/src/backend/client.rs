//! An attached control client. The engine only sees lines in and commands
//! out, whichever backend attached it.

use std::pin::Pin;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt};
use tokio::task::JoinHandle;

pub type Reader = Pin<Box<dyn AsyncBufRead + Send>>;
pub type Writer = Pin<Box<dyn AsyncWrite + Send>>;

/// How long a backend gets to let a closing client go.
const GOODBYE: Duration = Duration::from_secs(2);

/// An attached control client: read control-mode lines from `reader`, write
/// commands to `writer`, and [`ControlClient::close`] to detach.
pub struct ControlClient {
    /// Parser selected by the backend; its events carry unescaped replies.
    pub parser: crate::control::Parser,
    pub reader: Reader,
    pub writer: Writer,
    /// When present, finishes when the backend has let the client go.
    pub watcher: Option<JoinHandle<()>>,
}

impl ControlClient {
    /// Detaches: closing our end of the client's input ends it. Gives the
    /// backend a moment to say goodbye, then drops the connection regardless,
    /// which the backend also takes as goodbye.
    pub async fn close(self) -> Result<()> {
        let ControlClient {
            reader,
            mut writer,
            watcher,
            parser: _,
        } = self;
        let _ = writer.shutdown().await;
        drop(writer);
        if let Some(mut watcher) = watcher
            && tokio::time::timeout(GOODBYE, &mut watcher).await.is_err()
        {
            watcher.abort();
        }
        drop(reader);
        Ok(())
    }
}
