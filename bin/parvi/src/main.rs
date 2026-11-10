//! `parvi`: the node, and the CLI over its commands.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use parvi_node_kit::identity::Identity;
use parvi_node_kit::{Build, Config, Options, cli, config, hook, mcp};

/// The node's areas, by name, in the order they are built. Adding an area
/// adds one line here and one in Cargo.toml.
const AREAS: &[(&str, Build)] = &[
    (parvi_terminals::NAME, parvi_terminals::build),
    (parvi_scheduled::NAME, parvi_scheduled::build),
];

const USAGE: &str = "usage:
  parvi serve [--config file] [--listen addr:port]
  parvi mcp [--node addr:port]
  parvi <noun> <verb> [args] [--json] [--node id|url]   the node's commands, e.g. parvi session list
  parvi hook <url> [--then command...] [payload]        (run by a program's own hook)";

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let args: Vec<String> = args.collect();
    match command.as_str() {
        hook::VERB => {
            hook::report(&args);
            Ok(())
        }
        "" | "-h" | "--help" | "help" => {
            println!("{USAGE}");
            match cli::overview(None).await {
                Ok(commands) => println!(
                    "\nCommands from the running node (parvi <noun> <verb> --help):\n{commands}"
                ),
                Err(error) => println!("\nThe node's commands show here while it runs: {error:#}"),
            }
            Ok(())
        }
        "serve" => serve(&args).await,
        "mcp" => {
            let node = match args.as_slice() {
                [] => parvi_node_kit::address::local(),
                [flag, node] if flag == "--node" => node.clone(),
                _ => bail!("{USAGE}"),
            };
            mcp::stdio(&node).await
        }
        noun if noun.starts_with('-') => bail!("{USAGE}"),
        noun => {
            let code = cli::run(noun, &args, &mut std::io::stdout()).await?;
            std::process::exit(code);
        }
    }
}

/// Serves until Ctrl-C or SIGTERM.
async fn serve(args: &[String]) -> Result<()> {
    let dir = Identity::config_dir()?;
    let mut file = dir.join(config::FILE);
    let mut listen = None;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let mut value = || args.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--config" => file = PathBuf::from(value()?),
            "--listen" => listen = Some(value()?.clone()),
            _ => bail!("{USAGE}"),
        }
    }
    let config = Config::load(&file)?;
    let running = parvi_node_kit::start(
        Options {
            dir,
            config,
            listen,
        },
        AREAS,
    )
    .await?;
    stopped().await;
    running.stop().await
}

async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
