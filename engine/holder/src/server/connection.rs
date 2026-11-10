//! One client connection: the greeting, then tmux control mode.

use std::sync::Arc;

use anyhow::{Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::NamedPipeServer;

use super::commands;
use super::holder::Holder;
use crate::handshake;
use crate::words::split;

pub async fn serve(holder: Arc<Holder>, pipe: NamedPipeServer) -> Result<()> {
    let (read, mut write) = tokio::io::split(pipe);
    let mut lines = BufReader::new(read);
    let mut line = String::new();
    lines.read_line(&mut line).await?;
    match handshake::answer(line.trim_end(), std::process::id()) {
        Ok(greeting) => write.write_all(greeting.as_bytes()).await?,
        Err(refusal) => {
            write.write_all(refusal.as_bytes()).await?;
            write.flush().await?;
            bail!("refused {:?}", line.trim_end());
        }
    }

    let client = holder.connect();
    let sender = client.clone();
    let mut writer = tokio::spawn(async move {
        while let Some(batch) = sender.outbox.next().await {
            if write.write_all(&batch).await.is_err() || write.flush().await.is_err() {
                break;
            }
        }
    });
    // tmux numbers a client's commands from 1; the engine only needs order.
    let mut number = 0u64;
    let read = async {
        loop {
            line.clear();
            if lines.read_line(&mut line).await? == 0 {
                return anyhow::Ok(());
            }
            let words = split(line.trim_end_matches(['\r', '\n']));
            if !words.is_empty() {
                number += 1;
                if commands::run(&holder, &client, number, &words) {
                    number += 1;
                }
            }
        }
    };
    // The connection is over when the client hangs up, its pipe breaks, or
    // it is dropped as too far behind.
    let (result, writing) = tokio::select! {
        result = read => (result, true),
        _ = &mut writer => (Ok(()), false),
        () = client.outbox.dropped() => {
            writer.abort();
            (Ok(()), false)
        }
    };
    holder.disconnect(&client);
    if writing {
        let _ = writer.await;
    }
    result
}
