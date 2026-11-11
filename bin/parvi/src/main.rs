//! `parvi`: the node, and the CLI over its commands.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use parvi::{AREAS, install};
use parvi_node_kit::identity::Identity;
use parvi_node_kit::{Config, Options, cli, config, hook, listing, mcp};

const USAGE: &str = "usage:
  parvi serve [--config file] [--listen addr:port]
  parvi mcp [--node addr:port]
  parvi <noun> <verb> [args] [--json] [--node id|url]   the node's commands, e.g. parvi session list
  parvi hook <url> [--then command...] [payload]        (run by a program's own hook)
  parvi kinds                                           who owns which frame kinds
  parvi install [--root dir] [--unit name] [--listen addr:port] ...
                                                        this build as the systemd user unit
  parvi upgrade [--from file] [--unit name] ...         replace the installed build; sessions stay
  parvi version [--node addr:port]                      this build's version and the node's
  parvi --version                                       this build's version";

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
        "install" | "upgrade" => std::process::exit(install::run(&command, &args).await),
        "--version" => {
            println!("parvi {}", parvi_node_kit::VERSION);
            Ok(())
        }
        "version" => version(&args).await,
        "kinds" => {
            print!("{}", listing::kinds_table(AREAS));
            Ok(())
        }
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

/// This build's version, and the version of the node that answers.
async fn version(args: &[String]) -> Result<()> {
    let node = match args {
        [] => parvi_node_kit::address::local(),
        [flag, node] if flag == "--node" => node.clone(),
        _ => bail!("{USAGE}"),
    };
    println!("parvi {}", parvi_node_kit::VERSION);
    let url = match node.contains("://") {
        true => node.clone(),
        false => format!("http://{node}"),
    };
    let client = parvi_node_kit::NodeClient::new(Some(&format!("{}/", url.trim_end_matches('/'))))?;
    match client
        .request(axum::http::Method::GET, "/api/health", None, None)
        .await
    {
        Ok((health, _)) => println!(
            "node {} on {node} runs {}",
            health["node"].as_str().unwrap_or("?"),
            health["version"].as_str().unwrap_or("?")
        ),
        Err(error) => println!("no node answers on {node}: {error:#}"),
    }
    Ok(())
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
