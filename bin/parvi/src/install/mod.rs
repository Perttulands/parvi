//! `parvi install` and `parvi upgrade`: this machine's node as a systemd
//! user unit (Linux and WSL).
//!
//! They live in the binary, not in an area: they put the binary itself in
//! place and run it, before any node exists or while the node they replace
//! stops, so no node area can own them. For the same reason they are CLI
//! verbs and no MCP tool: an upgrade run by the node being upgraded would
//! be stopped by its own restart, and its caller would lose the answer.
//!
//! Sessions survive both: tmux holds them, and the unit's
//! `KillMode=process` lets a restart signal only the node.

mod unit;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use axum::http::Method;
use parvi_core::Error;
use parvi_node_kit::identity::{Environment, Identity};
use parvi_node_kit::{Config, NodeClient, config};
use serde_json::{Value, json};

pub use unit::Unit;

pub const INSTALL_USAGE: &str = "usage: parvi install [--from file] [--app file] [--root dir] \
[--unit name] [--unit-dir dir] [--listen addr:port] [--config file] [--timeout seconds] [--json]";
pub const UPGRADE_USAGE: &str = "usage: parvi upgrade [--from file] [--app file] [--unit name] \
[--unit-dir dir] [--timeout seconds] [--json]";

/// Where an install lives and how it is reached.
struct Flags {
    from: Option<PathBuf>,
    app: Option<PathBuf>,
    root: Option<PathBuf>,
    unit: String,
    unit_dir: Option<PathBuf>,
    listen: Option<String>,
    config: Option<PathBuf>,
    timeout: Duration,
    json: bool,
}

/// Runs `parvi install` or `parvi upgrade` (`verb`) with `args`, printing
/// what it did, or its error as `{code, message}`. Answers the exit code.
pub async fn run(verb: &str, args: &[String]) -> i32 {
    let json = args.iter().any(|a| a == "--json");
    let done = match parse(verb, args) {
        Ok(flags) if verb == "install" => install(&flags).await,
        Ok(flags) => upgrade(&flags).await,
        Err(error) => Err(error),
    };
    match done {
        Ok(report) if json => {
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            0
        }
        Ok(report) => {
            println!("{}", report["summary"].as_str().unwrap_or_default());
            0
        }
        Err(error) if json => {
            eprintln!("{}", json!({ "error": error }));
            1
        }
        Err(error) => {
            eprintln!("parvi {verb}: {error}");
            1
        }
    }
}

fn parse(verb: &str, args: &[String]) -> Result<Flags, Error> {
    let usage = if verb == "install" {
        INSTALL_USAGE
    } else {
        UPGRADE_USAGE
    };
    let bad = |why: String| Error::new("bad_arguments", format!("{why}\n{usage}"));
    let mut flags = Flags {
        from: None,
        app: None,
        root: None,
        unit: "parvi".into(),
        unit_dir: None,
        listen: None,
        config: None,
        timeout: Duration::from_secs(30),
        json: false,
    };
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        if flag == "--json" {
            flags.json = true;
            continue;
        }
        let only_install = matches!(flag.as_str(), "--root" | "--listen" | "--config");
        if only_install && verb != "install" {
            return Err(bad(format!(
                "{flag}: parvi upgrade keeps what the unit says; parvi install changes it"
            )));
        }
        let value = args
            .next()
            .ok_or_else(|| bad(format!("{flag} needs a value")))?;
        match flag.as_str() {
            "--from" => flags.from = Some(value.into()),
            "--app" => flags.app = Some(value.into()),
            "--root" => flags.root = Some(value.into()),
            "--unit" => flags.unit = value.clone(),
            "--unit-dir" => flags.unit_dir = Some(value.into()),
            "--listen" => flags.listen = Some(value.clone()),
            "--config" => flags.config = Some(value.into()),
            "--timeout" => {
                let seconds: f64 = value
                    .parse()
                    .map_err(|_| bad(format!("--timeout {value}: not a number of seconds")))?;
                flags.timeout = Duration::from_secs_f64(seconds);
            }
            _ => return Err(bad(format!("unknown flag {flag}"))),
        }
    }
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@');
    if flags.unit.is_empty() || !flags.unit.chars().all(plain) {
        return Err(bad(format!(
            "--unit {}: a unit name is letters, digits and -_.@",
            flags.unit
        )));
    }
    Ok(flags)
}

fn linux_only() -> Result<(), Error> {
    if cfg!(target_os = "linux") {
        Ok(())
    } else {
        Err(Error::new(
            "unsupported",
            "parvi install and upgrade run the node as a systemd user unit, on Linux and WSL",
        ))
    }
}

fn home() -> Result<PathBuf, Error> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| Error::new("no_home", "HOME is not set"))
}

/// The unit file: `<unit-dir>/<unit>.service`, the unit dir being
/// `$XDG_CONFIG_HOME/systemd/user` or `~/.config/systemd/user`.
fn unit_file(flags: &Flags) -> Result<PathBuf, Error> {
    let dir = match &flags.unit_dir {
        Some(dir) => dir.clone(),
        None => match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            Some(config) => PathBuf::from(config).join("systemd/user"),
            None => home()?.join(".config/systemd/user"),
        },
    };
    Ok(dir.join(format!("{}.service", flags.unit)))
}

/// The build to install: `--from`, or the `parvi` running this command.
fn source(flags: &Flags) -> Result<PathBuf, Error> {
    let from = match &flags.from {
        Some(from) => from.clone(),
        None => std::env::current_exe()
            .map_err(|e| Error::new("io", format!("finding the running parvi: {e}")))?,
    };
    std::fs::canonicalize(&from)
        .map_err(|e| Error::new("no_binary", format!("{}: {e}", from.display())))
}

/// What `binary --version` says it is. Also proves the file runs.
fn version_of(binary: &Path) -> Result<String, Error> {
    let unrunnable = |why: String| {
        Error::new(
            "binary_unrunnable",
            format!("{} --version: {why}", binary.display()),
        )
    };
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .map_err(|e| unrunnable(e.to_string()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    match text.trim().strip_prefix("parvi ") {
        Some(version) if output.status.success() => Ok(version.to_owned()),
        _ => Err(unrunnable(format!(
            "answered {:?} ({})",
            text.trim(),
            output.status
        ))),
    }
}

/// `systemctl --user <args>`, the `systemctl` on PATH. Answers its stdout;
/// a failure is `systemctl_failed` with what it said.
fn systemctl(args: &[&str]) -> Result<String, Error> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| Error::new("systemctl_failed", format!("running systemctl: {e}")))?;
    if !output.status.success() {
        return Err(Error::new(
            "systemctl_failed",
            format!(
                "systemctl --user {}: {} {}",
                args.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// `active`, `inactive`, `failed`, `activating`... as systemd says. Not
/// an error when it is not active.
fn state(service: &str) -> String {
    Command::new("systemctl")
        .args(["--user", "is-active", service])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .ok()
        .filter(|state| !state.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn io(what: &str, path: &Path, error: std::io::Error) -> Error {
    Error::new("io", format!("{what} {}: {error}", path.display()))
}

/// Puts `from`'s bytes at `to` whole: written beside it, then renamed over
/// it, so `to` is never missing or partial and a node running the old file
/// keeps running it.
fn place(from: &Path, to: &Path) -> Result<(), Error> {
    let dir = to.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| io("creating", dir, e))?;
    let name = to.file_name().unwrap_or_default().to_string_lossy();
    let next = dir.join(format!(".{name}.{}.new", std::process::id()));
    std::fs::copy(from, &next)
        .map_err(|e| io(&format!("copying {} to", from.display()), &next, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| io("making executable", &next, e))?;
    }
    std::fs::rename(&next, to).map_err(|e| {
        let _ = std::fs::remove_file(&next);
        io("replacing", to, e)
    })
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn write_text(path: &Path, text: &str) -> Result<(), Error> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| io("creating", dir, e))?;
    let next = path.with_extension(format!("{}.new", std::process::id()));
    std::fs::write(&next, text).map_err(|e| io("writing", &next, e))?;
    std::fs::rename(&next, path).map_err(|e| io("writing", path, e))
}

/// Where to reach a node listening on `listen`: loopback for a wildcard.
fn health_url(listen: &str) -> String {
    let address = match listen.rsplit_once(':') {
        Some(("0.0.0.0", port)) => format!("127.0.0.1:{port}"),
        Some(("[::]", port)) => format!("[::1]:{port}"),
        _ => listen.to_owned(),
    };
    format!("http://{address}/")
}

/// Waits until the node on `listen` answers `/api/health` as `version`
/// (any version when `None`). On timeout, says what it saw last and the
/// unit's state.
async fn healthy(
    listen: &str,
    version: Option<&str>,
    service: &str,
    timeout: Duration,
) -> Result<(), String> {
    let client = NodeClient::new(Some(&health_url(listen))).map_err(|e| format!("{e:#}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        let asked = client.request(Method::GET, "/api/health", None, None);
        let last = match tokio::time::timeout(Duration::from_secs(2), asked).await {
            Ok(Ok((health, _))) => {
                let answered = health["version"].as_str().unwrap_or("none");
                if health["status"] == "ok" && version.is_none_or(|v| v == answered) {
                    return Ok(());
                }
                format!(
                    "it answers as version {answered}, status {}",
                    health["status"]
                )
            }
            Ok(Err(error)) => format!("{error:#}"),
            Err(_) => "/api/health did not answer within 2 s".into(),
        };
        if Instant::now() >= deadline {
            return Err(format!(
                "no healthy node on {listen} within {:.0?}: {last}; {service} is {}",
                timeout,
                state(service)
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The app's place beside the installed `parvi`.
fn app_path(binary: &Path) -> PathBuf {
    binary.with_file_name("parvi-app")
}

fn previous(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{name}.previous"))
}

async fn install(flags: &Flags) -> Result<Value, Error> {
    linux_only()?;
    let from = source(flags)?;
    let version = version_of(&from)?;
    let root = match &flags.root {
        Some(root) => root.clone(),
        None => home()?.join(".local"),
    };
    let binary = root.join("bin/parvi");
    let config_file = match &flags.config {
        Some(file) => std::path::absolute(file).map_err(|e| io("finding", file, e))?,
        None => Identity::config_dir()
            .map_err(|e| Error::new("no_home", format!("{e:#}")))?
            .join(config::FILE),
    };
    let listen = match &flags.listen {
        Some(listen) => listen.clone(),
        None => Config::load(&config_file)?
            .node()?
            .address(Environment::detect())?,
    };
    let mut environment = Vec::new();
    for name in ["PATH", "PARVI_TMUX"] {
        if let Some(value) = std::env::var_os(name).filter(|v| !v.is_empty()) {
            environment.push((name.to_owned(), value.to_string_lossy().into_owned()));
        }
    }
    let text = Unit {
        binary: binary.clone(),
        listen: listen.clone(),
        config: flags.config.as_ref().map(|_| config_file.clone()),
        environment,
    }
    .render();
    let unit_file = unit_file(flags)?;
    let service = format!("{}.service", flags.unit);

    let mut copies = vec![(from.clone(), binary.clone())];
    if let Some(app) = &flags.app {
        copies.push((app.clone(), app_path(&binary)));
    }
    let binaries_changed = copies.iter().any(|(from, to)| !same_bytes(from, to));
    let unit_changed = std::fs::read_to_string(&unit_file).ok().as_deref() != Some(&text);
    let was = state(&service);
    if was == "active" && binaries_changed {
        return Err(Error::new(
            "install_running",
            format!(
                "{service} is running another build at {}; parvi upgrade replaces a running \
                 node and puts it back if the new one fails",
                binary.display()
            ),
        ));
    }
    let mut did = Vec::new();
    if binaries_changed {
        for (from, to) in &copies {
            place(from, to)?;
            did.push(format!("put {} at {}", from.display(), to.display()));
        }
    }
    if unit_changed {
        write_text(&unit_file, &text)?;
        systemctl(&["daemon-reload"])?;
        did.push(format!("wrote {}", unit_file.display()));
    }
    systemctl(&["enable", "--quiet", &service])?;
    if was != "active" {
        systemctl(&["start", &service])?;
        did.push(format!("started {service}"));
    } else if unit_changed {
        systemctl(&["restart", &service])?;
        did.push(format!("restarted {service} with the new unit"));
    }
    healthy(&listen, Some(&version), &service, flags.timeout)
        .await
        .map_err(|why| {
            Error::new(
                "install_unhealthy",
                format!(
                    "{service} is installed and enabled, but {why}. \
                     See journalctl --user -u {service}"
                ),
            )
        })?;
    let config_note = if config_file.exists() {
        "its settings"
    } else {
        "not there yet; the node runs with defaults until it is"
    };
    let summary = format!(
        "{}\nParvi {version} serves on {listen} as {service} ({}).\nBinaries: {}\nConfig: {} ({config_note})",
        if did.is_empty() {
            "Already installed; nothing changed.".to_owned()
        } else {
            did.join("\n")
        },
        unit_file.display(),
        copies
            .iter()
            .map(|(_, to)| to.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        config_file.display(),
    );
    Ok(json!({
        "summary": summary,
        "changed": !did.is_empty(),
        "version": version,
        "unit": service,
        "unit_file": unit_file,
        "listen": listen,
        "binary": binary,
        "config": config_file,
    }))
}

async fn upgrade(flags: &Flags) -> Result<Value, Error> {
    linux_only()?;
    let unit_file = unit_file(flags)?;
    let service = format!("{}.service", flags.unit);
    let text = std::fs::read_to_string(&unit_file).map_err(|e| {
        Error::new(
            "not_installed",
            format!(
                "reading {}: {e}; parvi install puts the node in place first",
                unit_file.display()
            ),
        )
    })?;
    let unit = Unit::parse(&text, &unit_file)?;
    let binary = unit.binary.clone();
    let from = source(flags)?;
    if std::fs::canonicalize(&binary).is_ok_and(|installed| installed == from) {
        return Err(Error::new(
            "upgrade_same_binary",
            format!(
                "{} is the installed binary; run the new build's parvi upgrade, or give --from",
                from.display()
            ),
        ));
    }
    let version = version_of(&from)?;
    // The build it replaces, which a failed upgrade puts back. It need not
    // run: then a put-back node is healthy at any version.
    let old_version = version_of(&binary).ok();
    let mut copies = vec![(from.clone(), binary.clone())];
    if let Some(app) = &flags.app {
        copies.push((app.clone(), app_path(&binary)));
    }
    let mut kept = Vec::new();
    for (_, to) in &copies {
        if to.exists() {
            place(to, &previous(to))?;
            kept.push(to.clone());
        }
    }
    for (from, to) in &copies {
        place(from, to)?;
    }
    let restarted = systemctl(&["restart", &service]).map_err(|e| e.message);
    let failed = match restarted {
        Ok(_) => healthy(&unit.listen, Some(&version), &service, flags.timeout)
            .await
            .err(),
        Err(why) => Some(why),
    };
    let Some(why) = failed else {
        let from_version = old_version.as_deref().unwrap_or("an unknown build");
        // The new build is healthy, so nothing can need the old one again.
        for to in &kept {
            std::fs::remove_file(previous(to)).map_err(|e| io("removing", &previous(to), e))?;
        }
        return Ok(json!({
            "summary": format!(
                "Upgraded {service} from {from_version} to {version}; it serves on {}.",
                unit.listen
            ),
            "version": version,
            "previous": old_version,
            "unit": service,
            "listen": unit.listen,
            "binary": binary,
        }));
    };
    // Put the previous build back and start it again.
    let mut put_back = Ok(());
    for to in &kept {
        put_back = put_back.and_then(|()| place(&previous(to), to));
    }
    let back = match put_back {
        Ok(()) => match systemctl(&["restart", &service]) {
            Ok(_) => {
                healthy(
                    &unit.listen,
                    old_version.as_deref(),
                    &service,
                    flags.timeout,
                )
                .await
            }
            Err(error) => Err(error.message),
        },
        Err(error) => Err(error.message),
    };
    let old = old_version.as_deref().unwrap_or("the previous build");
    Err(match back {
        Ok(()) => Error::new(
            "upgrade_failed",
            format!(
                "the new build {version} failed: {why}. Put {old} back at {}; it serves again \
                 on {}. See journalctl --user -u {service}",
                binary.display(),
                unit.listen
            ),
        ),
        Err(again) => Error::new(
            "upgrade_rollback_failed",
            format!(
                "the new build {version} failed: {why}. Putting {old} back failed too: {again}. \
                 The previous binary is {}; see journalctl --user -u {service}",
                previous(&binary).display()
            ),
        ),
    })
}
