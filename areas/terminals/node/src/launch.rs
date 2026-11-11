//! Launching, renaming and deleting a session, and the server's mouse mode
//! (issues #20, #184): the launcher's host configuration, the command a
//! launch types, and the tmux commands for each.
//!
//! A launch creates the session detached in the folder, sizes it once with a
//! `resize-window` followed by unsetting `window-size` (ADR-0017 decision 3),
//! then types the harness's command line
//! into the session's shell, as CHROTE 2 does, so the shell outlives the
//! harness. Renaming and deleting act on exactly one session, by its id.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::agent_events::hooks::{self, Hooks};
use parvi_core::json::push_string as push_json_string;
use parvi_term::backend::Backend;
use parvi_term::engine::Engine;
pub use parvi_terminals_contract::Launch;
use std::sync::atomic::{AtomicU64, Ordering};

static LAUNCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The harness that runs nothing: the user's shell. Always offered, first.
pub const SHELL: &str = "shell";

/// How long one tmux command may take.
const TMUX_TIMEOUT: Duration = Duration::from_secs(5);

/// The most a flag line may hold, as in CHROTE 2.
const MAX_FLAGS: usize = 4096;

/// What a launcher offers. Nothing in it is secret, and nothing secret
/// belongs in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub harnesses: Vec<Harness>,
    pub folders: Vec<String>,
    /// The size a new session's window is given once, at creation.
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Harness {
    pub id: String,
    /// The program, empty for the shell.
    pub command: String,
    /// The flag line the launcher starts from.
    pub flags: String,
}

impl Default for Config {
    /// The shell in the home folder, at CHROTE 2's canonical 200x50.
    fn default() -> Config {
        Config {
            harnesses: vec![Harness {
                id: SHELL.into(),
                command: String::new(),
                flags: String::new(),
            }],
            folders: vec!["~".into()],
            cols: 200,
            rows: 50,
        }
    }
}

impl Config {
    /// Parses lines of `harness <id> <command> [default flags]`,
    /// `folder <path>` and `size <cols>x<rows>`. `#` starts a comment line.
    pub fn parse(text: &str) -> Result<Config> {
        let mut config = Config::default();
        let mut folders = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let at = || format!("line {}", number + 1);
            let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let rest = rest.trim();
            match key {
                "harness" => {
                    let mut words = rest.splitn(3, char::is_whitespace);
                    let id = words.next().unwrap_or("");
                    let command = words.next().unwrap_or("");
                    let flags = words.next().unwrap_or("").trim();
                    if id.is_empty()
                        || !id
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    {
                        bail!("{}: a harness id is lowercase letters, digits and -", at());
                    }
                    if id == SHELL {
                        bail!("{}: the shell harness is built in", at());
                    }
                    if command.is_empty() {
                        bail!("{}: harness {id} needs a command", at());
                    }
                    if config.harnesses.iter().any(|h| h.id == id) {
                        bail!("{}: harness {id} is named twice", at());
                    }
                    check_flags(flags).with_context(at)?;
                    config.harnesses.push(Harness {
                        id: id.into(),
                        command: command.into(),
                        flags: flags.into(),
                    });
                }
                "folder" => {
                    if !(rest == "~"
                        || rest.starts_with("~/")
                        || rest.starts_with("~\\")
                        || Path::new(rest).is_absolute())
                    {
                        bail!("{}: a folder is absolute or starts with ~", at());
                    }
                    folders.push(rest.to_owned());
                }
                "size" => {
                    let size = rest
                        .split_once('x')
                        .and_then(|(c, r)| Some((c.parse::<u16>().ok()?, r.parse::<u16>().ok()?)));
                    match size {
                        Some((cols, rows)) if cols >= 80 && rows >= 24 => {
                            (config.cols, config.rows) = (cols, rows);
                        }
                        _ => bail!("{}: size is <cols>x<rows>, at least 80x24", at()),
                    }
                }
                _ => bail!("{}: unknown setting {key:?}", at()),
            }
        }
        if !folders.is_empty() {
            config.folders = folders;
        }
        Ok(config)
    }

    /// `{"harnesses":[{"id":…,"command":…,"flags":…}],"folders":[…],"cols":…,"rows":…}`
    pub fn json(&self) -> String {
        let mut json = String::from("{\"harnesses\":[");
        for (i, harness) in self.harnesses.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str("{\"id\":");
            push_json_string(&mut json, &harness.id);
            json.push_str(",\"command\":");
            push_json_string(&mut json, &harness.command);
            json.push_str(",\"flags\":");
            push_json_string(&mut json, &harness.flags);
            json.push('}');
        }
        json.push_str("],\"folders\":[");
        for (i, folder) in self.folders.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            push_json_string(&mut json, folder);
        }
        json.push_str(&format!(
            "],\"cols\":{},\"rows\":{}}}",
            self.cols, self.rows
        ));
        json
    }
}

/// What a launch types into the new session's shell: the command and the
/// flag line. Empty for the shell. The launcher's preview shows exactly this
/// (`ui/src/launch.ts` builds the same string).
pub fn command_line(command: &str, flags: &str) -> String {
    match (command.trim(), flags.trim()) {
        ("", _) => String::new(),
        (command, "") => command.to_owned(),
        (command, flags) => format!("{command} {flags}"),
    }
}

/// A session name tmux targets unambiguously: letters, digits, `_` and `-`,
/// at most 50, as in CHROTE 2.
pub fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 50 {
        bail!("a session name is 1 to 50 characters");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        bail!("a session name is letters, digits, _ and -");
    }
    Ok(())
}

/// A Unix user name as `useradd` takes it, so it reads plainly in a session
/// id like `$3@alice` and in the browser.
pub fn check_user(user: &str) -> Result<()> {
    let ok = !user.is_empty()
        && user.len() <= 32
        && !user.starts_with('-')
        && user
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !ok {
        bail!("{user:?} is not a user name: lowercase letters, digits, _ and -");
    }
    Ok(())
}

fn check_flags(flags: &str) -> Result<()> {
    if flags.len() > MAX_FLAGS {
        bail!("the flag line is longer than {MAX_FLAGS} bytes");
    }
    if flags.chars().any(char::is_control) {
        bail!("the flag line holds a control character");
    }
    Ok(())
}

/// `~` and `~/…` under `home`; absolute paths as they are. Checked to be a
/// folder only if `check`: another user's folders may be closed to us.
fn resolve_folder(folder: &str, home: Option<&Path>, check: bool) -> Result<PathBuf> {
    let folder = folder.trim();
    let path = if folder.is_empty() || folder == "~" {
        home.context("no home folder to start in")?.to_owned()
    } else if let Some(rest) = folder
        .strip_prefix("~/")
        .or_else(|| folder.strip_prefix("~\\"))
    {
        home.context("no home folder to start in")?.join(rest)
    } else if Path::new(folder).is_absolute() {
        PathBuf::from(folder)
    } else {
        bail!("a folder is absolute or starts with ~");
    };
    if check && !path.is_dir() {
        bail!("{} is not a folder", path.display());
    }
    Ok(path)
}

/// A session a launch made, and what did not go as asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    /// The new session's id, like `$7`.
    pub id: String,
    /// Why it runs without the completion hooks it was asked for.
    pub warning: Option<String>,
}

/// Creates the session, sizes it once and starts the harness. A session that
/// can't be sized is killed again; one whose harness can't be typed is kept,
/// with a warning.
///
/// For another user (`launch.user` set), `backend` is that user's server. It
/// must be running as them; `~` is their home as their server knows it, and
/// the folder is left to tmux to find.
///
/// `hooks` is how this node hooks a harness, or why it can't; they are
/// asked for with `launch.notify`, only on the service user's own server,
/// whose ids are the hub's. They go before the flag line; flags that would
/// override them stop them being installed rather than lose to them. They
/// are settled before the session exists, as CHROTE settles them. Hooks that
/// can't be installed never stop a launch: it runs without them and says
/// why.
pub async fn launch(
    engine: &Engine,
    config: &Config,
    launch: &Launch,
    hooks: Result<&Hooks, &str>,
) -> Result<Launched> {
    let backend = engine.backend().as_ref();
    let mut changes = engine.watch();
    let harness = config
        .harnesses
        .iter()
        .find(|h| h.id == launch.harness)
        .with_context(|| format!("no harness {:?}", launch.harness))?;
    check_name(&launch.name)?;
    check_flags(&launch.flags)?;
    let own = launch.user.is_empty();
    let home = if own {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
    } else {
        check_owner(backend, &launch.user).await?;
        server_home(backend).await
    };
    let folder = resolve_folder(&launch.folder, home.as_deref(), own)?;
    let mut warning = None;
    let plan = match hooks {
        _ if !launch.notify || !own => None,
        Err(why) if hooks::takes_hooks(&harness.command) => {
            warning = Some(why.to_owned());
            None
        }
        Err(_) => None,
        Ok(hooks) => match hooks.plan(&harness.command, &launch.flags) {
            Ok(plan) => plan.map(|plan| (hooks, plan)),
            Err(why) => {
                warning = Some(why);
                None
            }
        },
    };

    let mut new = vec!["new-session", "-d", "-P", "-F", "#{session_id}", "-s"];
    new.push(&launch.name);
    new.push("-c");
    let folder = folder.to_string_lossy();
    new.push(&folder);
    let channel = format!(
        "parvi-launch-{}-{}",
        std::process::id(),
        LAUNCH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let gate = backend.launch_gate(&channel);
    #[cfg(unix)]
    if gate.is_none() {
        bail!("cannot prepare the tmux launch gate");
    }
    if let Some(command) = &gate {
        new.push(command);
    }
    let id = run(backend, &new).await?.trim().to_owned();
    if !is_session_id(&id) {
        bail!("backend answered {id:?} for the new session's id");
    }

    let (cols, rows) = launch.size.unwrap_or((config.cols, config.rows));
    let sizing = tokio::time::timeout(TMUX_TIMEOUT, backend.size(&id, cols, rows)).await;
    let sizing = match sizing {
        Ok(result) => result,
        Err(error) => Err(error.into()),
    };
    if let Err(error) = sizing {
        let _ = run(backend, &["kill-session", "-t", &id]).await;
        return Err(error.context("sizing the new session"));
    }

    let mut attach_warning = None;
    let pane = if gate.is_some() {
        let seeded = async {
            loop {
                if let Some(pane) = engine
                    .snapshot()
                    .sessions
                    .iter()
                    .find(|s| s.id == id)
                    .and_then(|s| s.panes.first())
                {
                    // Launch needs the shell's output before releasing the
                    // gate, so it is a consumer of this pane.
                    if engine.text(pane.id).is_some() {
                        return Ok::<_, anyhow::Error>(pane.id);
                    }
                }
                changes.changed().await.context("engine stopped")?;
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(8), seeded).await;
        // Signal even on failure; the pane also has its own timeout fallback.
        let _ = run(backend, &["wait-for", "-S", &channel]).await;
        match result {
            Ok(Ok(pane)) => Some(pane),
            _ => {
                attach_warning =
                    Some("the terminal engine did not attach before shell startup".into());
                None
            }
        }
    } else {
        None
    };

    let hooked = plan.and_then(|(hooks, plan)| match hooks.flags(&plan, &id) {
        Ok(flags) => Some(flags),
        Err(error) => {
            warning = Some(format!("{error:#}"));
            None
        }
    });
    let flags = match hooked {
        Some(hooked) => format!("{hooked} {}", launch.flags),
        None => launch.flags.clone(),
    };
    let mut warning = warning.map(|why| format!("no completion notices: {why}"));
    if let Some(why) = attach_warning {
        add_warning(&mut warning, why);
    }
    let line = command_line(&harness.command, &flags);
    if !line.is_empty() {
        let typed = async {
            if gate.is_some() && pane.is_none() {
                bail!("the terminal engine did not attach before shell startup");
            }
            if let Some(pane) = pane {
                engine
                    .deliver_when_ready(pane, line, true, Duration::from_secs(30))
                    .await?
                    .await
            } else {
                run(backend, &["send-keys", "-t", &id, "-l", &line]).await?;
                run(backend, &["send-keys", "-t", &id, "Enter"])
                    .await
                    .map(|_| ())
            }
        };
        if let Err(error) = typed.await {
            // The session is a working shell in the folder: it stays, as in
            // CHROTE, and the operator is told what did not start.
            let reason = if error.is::<parvi_term::engine::NotReady>() {
                format!(
                    "{error:#}; the shell may never have enabled bracketed paste (bash older than 5.1, enable-bracketed-paste off in .inputrc, sh/dash, or the launch gate's timeout command missing)"
                )
            } else {
                format!("{error:#}")
            };
            let failure = format!("{} did not start: {reason}", harness.id);
            add_warning(&mut warning, failure);
        }
    }
    Ok(Launched { id, warning })
}

fn add_warning(warning: &mut Option<String>, next: String) {
    match warning {
        Some(existing) => {
            existing.push_str("; ");
            existing.push_str(&next);
        }
        None => *warning = Some(next),
    }
}

/// Refuses unless `user`'s server is running on the backend. With no server
/// there, `new-session` would start one as the service user, and the session
/// would run as us rather than as them.
async fn check_owner(backend: &dyn Backend, user: &str) -> Result<()> {
    let owner = run(backend, &["display-message", "-p", "#{user}"])
        .await
        .with_context(|| format!("{user}'s tmux server is not running"))?;
    let owner = owner.trim();
    if owner != user {
        bail!("the tmux server for {user} runs as {owner:?}");
    }
    Ok(())
}

/// The home folder a tmux server's global environment holds.
async fn server_home(backend: &dyn Backend) -> Option<PathBuf> {
    let line = run(backend, &["show-environment", "-g", "HOME"])
        .await
        .ok()?;
    let home = line.trim().strip_prefix("HOME=")?;
    home.starts_with('/').then(|| PathBuf::from(home))
}

/// tmux's right-click menus, which take a right click from the app's own
/// menu while the mouse is on. CHROTE removes them wherever it sets the mouse.
const RIGHT_CLICK_MENUS: [&str; 6] = [
    "MouseDown3Pane",
    "MouseDown3Status",
    "MouseDown3StatusLeft",
    "M-MouseDown3Pane",
    "M-MouseDown3Status",
    "M-MouseDown3StatusLeft",
];

/// Turns tmux's mouse mode on or off for the whole server, as CHROTE's
/// mouse toggle does, and removes tmux's right-click menus. A Windows holder
/// has no mouse option and refuses.
pub async fn mouse(backend: &dyn Backend, on: bool) -> Result<()> {
    let value = if on { "on" } else { "off" };
    run(backend, &["set-option", "-g", "mouse", value]).await?;
    for key in RIGHT_CLICK_MENUS {
        // -q: a binding already gone is no error.
        run(backend, &["unbind-key", "-q", "-n", key]).await?;
    }
    Ok(())
}

/// Kills exactly the session with this id.
pub async fn delete(backend: &dyn Backend, session: &str) -> Result<()> {
    if !is_session_id(session) {
        bail!("{session:?} is not a session id");
    }
    run(backend, &["kill-session", "-t", session]).await?;
    Ok(())
}

/// Renames exactly the session with this id. tmux refuses a name another
/// session on its server has; a Windows holder refuses any rename.
pub async fn rename(backend: &dyn Backend, session: &str, name: &str) -> Result<()> {
    if !is_session_id(session) {
        bail!("{session:?} is not a session id");
    }
    check_name(name)?;
    run(backend, &["rename-session", "-t", session, name]).await?;
    Ok(())
}

fn is_session_id(id: &str) -> bool {
    id.strip_prefix('$')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Runs a command on the backend, returning its reply.
async fn run(backend: &dyn Backend, args: &[&str]) -> Result<String> {
    tokio::time::timeout(TMUX_TIMEOUT, backend.command(args))
        .await
        .with_context(|| format!("command {} took too long", args[0]))?
        .with_context(|| format!("command {}", args[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_warnings_keep_hooks_and_attachment_failures() {
        let mut warning = Some("no completion notices: hooks failed".to_owned());
        add_warning(
            &mut warning,
            "the terminal engine did not attach before shell startup".into(),
        );
        add_warning(&mut warning, "harness did not start".into());
        assert_eq!(
            warning.as_deref(),
            Some(
                "no completion notices: hooks failed; the terminal engine did not attach before shell startup; harness did not start"
            )
        );
    }

    #[test]
    fn parses_harnesses_folders_and_size() {
        let config = Config::parse(
            "# Parvi launcher\n\
             harness claude-code claude --permission-mode acceptEdits\n\
             harness codex codex\n\
             \n\
             folder ~/work/api\n\
             folder ~/agents\n\
             size 160x48\n",
        )
        .unwrap();
        let ids: Vec<&str> = config.harnesses.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["shell", "claude-code", "codex"]);
        assert_eq!(config.harnesses[1].command, "claude");
        assert_eq!(config.harnesses[1].flags, "--permission-mode acceptEdits");
        assert_eq!(config.harnesses[2].flags, "");
        assert_eq!(config.folders, ["~/work/api", "~/agents"]);
        assert_eq!((config.cols, config.rows), (160, 48));
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[cfg(windows)]
    #[test]
    fn accepts_windows_folders() {
        let config = Config::parse("folder C:\\work\\api\nfolder ~\\work").unwrap();
        assert_eq!(config.folders, [r"C:\work\api", r"~\work"]);
    }

    #[test]
    fn refuses_what_it_cannot_mean() {
        for (text, why) in [
            ("harness Claude claude", "lowercase"),
            ("harness shell bash", "built in"),
            ("harness codex", "needs a command"),
            ("harness a x\nharness a y", "twice"),
            ("folder work", "absolute"),
            ("size 40x10", "at least"),
            ("colour red", "unknown"),
        ] {
            let error = format!("{:#}", Config::parse(text).unwrap_err());
            assert!(error.contains(why), "{text:?}: {error}");
        }
    }

    #[test]
    fn writes_the_launcher_as_json() {
        let config = Config::parse("harness x run \"a b\"\nfolder ~").unwrap();
        assert_eq!(
            config.json(),
            r#"{"harnesses":[{"id":"shell","command":"","flags":""},{"id":"x","command":"run","flags":"\"a b\""}],"folders":["~"],"cols":200,"rows":50}"#
        );
    }

    #[test]
    fn builds_the_command_line_the_preview_shows() {
        assert_eq!(command_line("claude", "  --a b "), "claude --a b");
        assert_eq!(command_line("codex", ""), "codex");
        assert_eq!(command_line("", "--ignored"), "");
    }

    #[test]
    fn checks_names_flags_and_ids() {
        assert!(check_name("claude-api_2").is_ok());
        for bad in ["", "has space", "a.b", "x:y", &"n".repeat(51)] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        assert!(check_user("alice_2").is_ok());
        for bad in ["", "Alice", "-x", "a@b", "a b"] {
            assert!(check_user(bad).is_err(), "{bad:?}");
        }
        assert!(check_flags("--x 'y z'").is_ok());
        assert!(check_flags("--x\n--y").is_err());
        assert!(is_session_id("$12"));
        assert!(!is_session_id("$"));
        assert!(!is_session_id("alpha"));
        assert!(!is_session_id("$1;kill-server"));
    }

    #[test]
    fn resolves_folders_under_home() {
        let home = std::env::temp_dir();
        assert_eq!(resolve_folder("~", Some(&home), true).unwrap(), home);
        assert_eq!(resolve_folder("", Some(&home), true).unwrap(), home);
        assert_eq!(
            resolve_folder("~/a folder", Some(&home), false).unwrap(),
            home.join("a folder")
        );
        #[cfg(windows)]
        assert_eq!(
            resolve_folder(r"C:\work\my checkout", Some(&home), false).unwrap(),
            PathBuf::from(r"C:\work\my checkout")
        );
        assert!(resolve_folder("relative", Some(&home), true).is_err());
        assert!(resolve_folder("~/surely-not-here-parvi", Some(&home), true).is_err());
        // Another user's folder is left to their tmux server.
        assert!(resolve_folder("~/surely-not-here-parvi", Some(&home), false).is_ok());
        assert!(resolve_folder("~", None, false).is_err());
    }
}
