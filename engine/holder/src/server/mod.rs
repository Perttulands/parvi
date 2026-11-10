//! The holder process: `parvi-holder [--pipe <path>] [--log <path>]`.
//!
//! It owns every session of this user and serves them on a named pipe until
//! it is told `kill-server`, or Windows ends it at log off.

mod client;
mod commands;
mod connection;
mod holder;
mod log;
mod session;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::sys::{conpty, pipe};
use holder::Holder;
use log::Log;

/// Where the holder logs when not told: `%LOCALAPPDATA%\Parvi\holder.log`.
fn default_log() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("TEMP"))
        .unwrap_or_else(|| ".".into());
    PathBuf::from(base).join("Parvi").join("holder.log")
}

struct Options {
    pipe: Option<String>,
    log: PathBuf,
}

fn options(args: impl IntoIterator<Item = String>) -> Result<Options> {
    let mut options = Options {
        pipe: None,
        log: default_log(),
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--pipe" => options.pipe = Some(value()?),
            "--log" => options.log = value()?.into(),
            other => bail!("unknown argument {other:?}"),
        }
    }
    Ok(options)
}

/// Runs the holder; returns the process exit code.
pub fn main() -> i32 {
    let options = match options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(e) => {
            Log::new(default_log()).line(&format!("{e:#}"));
            return 2;
        }
    };
    let log = Log::new(options.log);
    let path = match options
        .pipe
        .map_or_else(|| pipe::user_sid().map(|sid| pipe::path(&sid)), Ok)
    {
        Ok(path) => path,
        Err(e) => {
            log.line(&format!("naming the pipe: {e:#}"));
            return 1;
        }
    };
    let holder = Holder::new(log);
    match serve(&holder, &path) {
        Ok(()) => 0,
        Err(e) => {
            holder.log.line(&format!("{path}: {e:#}"));
            1
        }
    }
}

fn serve(holder: &Arc<Holder>, path: &str) -> Result<()> {
    conpty::let_children_take_ctrl_c();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut listener = match pipe::listen(path, true) {
            Ok(listener) => listener,
            Err(e) => {
                // One holder per pipe: the first instance is taken.
                holder.log.line(&format!("not serving {path}: {e}"));
                return Ok(());
            }
        };
        holder.log.line(&format!("serving {path}"));
        loop {
            listener.connect().await?;
            let connected = std::mem::replace(&mut listener, pipe::listen(path, false)?);
            let holder = holder.clone();
            tokio::spawn(async move {
                if let Err(e) = connection::serve(holder.clone(), connected).await {
                    holder.log.line(&format!("connection: {e:#}"));
                }
            });
        }
    })
}
