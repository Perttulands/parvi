//! Installing a harness's completion hooks at launch. Each runs
//! `parvi hook` (`parvi_node_kit::hook`) with the address to report to.
//!
//! Each harness is told its own way, as CHROTE tells it: Claude Code reads a
//! Stop and a Notification hook from a settings file named with
//! `--settings`; Codex runs a notify program given in a `-c` override and
//! has no waiting event. The hook runs this binary, which posts to the node
//! with the session's hub id in the address, so it needs nothing from the
//! session's environment and works the same under tmux and the Windows
//! holder. The line is typed into the session's shell, a POSIX shell or
//! PowerShell, so every word is one both read alike: single-quoted, or
//! double-quoted with nothing either would expand. Codex's notify override
//! is quoted for the one shell it is typed into, so its TOML arrives as
//! written ([`codex_notify_flag`]).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use parvi_node_kit::hook::{THEN, VERB};
use serde_json::json;

use super::{Kind, PATH};

/// Claude Code's ceiling on each hook command, above the hook's own two
/// seconds to report.
const CLAUDE_HOOK_SECONDS: u64 = 5;

/// How long a settings file is kept for a session not yet listed.
const JUST_WRITTEN: Duration = Duration::from_secs(60);

/// Characters a hook's words may not hold: they would end a quote or be
/// expanded by one of the shells the line is typed into.
const UNSAFE: &[char] = &['\'', '"', '$', '`', '!', '\n', '\r'];

/// What a launch needs to wire hooks to this node.
#[derive(Debug, Clone)]
pub struct Hooks {
    /// This binary.
    exe: String,
    /// `http://<addr>/api/agent/event`, as a process on this host reaches it.
    url: String,
    /// Where Claude Code's settings files go, one per session.
    dir: PathBuf,
    /// Codex's own config, whose `notify` a Codex launch passes on.
    codex_config: PathBuf,
}

/// The harness a command runs, if it is one that takes hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Harness {
    Claude,
    Codex,
}

fn harness(command: &str) -> Option<Harness> {
    let name = Path::new(command.trim()).file_stem()?.to_str()?;
    match name {
        "claude" => Some(Harness::Claude),
        "codex" => Some(Harness::Codex),
        _ => None,
    }
}

/// Whether the node hooks this harness command at all: Claude Code and Codex.
pub fn takes_hooks(command: &str) -> bool {
    harness(command).is_some()
}

/// The hooks one launch will get, settled before its session exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    harness: Harness,
    /// The operator's own Codex notify command, run after Parvi's report.
    then: Vec<String>,
}

/// The session's hub id as one query value: `$3@alice` is `%243%40alice`.
fn encode(session: &str) -> String {
    session
        .bytes()
        .map(|b| match b {
            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Codex's config file: `$CODEX_HOME/config.toml`, else `.codex` in the home
/// folder.
pub fn codex_config() -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
            Some(PathBuf::from(home).join(".codex"))
        })?;
    Some(home.join("config.toml"))
}

/// The `notify` command Codex's config gives, if any.
fn codex_notify(config: &Path) -> Result<Vec<String>> {
    let text = match std::fs::read_to_string(config) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).with_context(|| format!("reading {}", config.display())),
    };
    let doc: toml::Table =
        toml::from_str(&text).with_context(|| format!("{} does not parse", config.display()))?;
    match doc.get("notify") {
        None => Ok(vec![]),
        Some(notify) => notify
            .as_array()
            .and_then(|a| a.iter().map(|w| w.as_str().map(str::to_owned)).collect())
            .with_context(|| format!("notify in {} is not a list of words", config.display())),
    }
}

/// Whether a Codex flag line sets `notify` itself (`-c notify=…`,
/// `--config notify=…`): a later `-c` wins, so it would replace Parvi's.
fn sets_codex_notify(flags: &str) -> bool {
    let flags = flags.replace(['\'', '"'], "");
    flags.match_indices("notify").any(|(at, word)| {
        let before = flags[..at].chars().next_back();
        let after = flags[at + word.len()..].trim_start();
        matches!(before, None | Some(' ' | '\t' | '=')) && after.starts_with('=')
    })
}

/// Whether a Claude Code flag line names its own `--settings` file.
fn sets_claude_settings(flags: &str) -> bool {
    flags
        .split_whitespace()
        .any(|w| w == "--settings" || w.starts_with("--settings="))
}

/// The `-c notify=[…]` flag carrying `words`, quoted for the shell the
/// session runs, so the TOML reaches Codex as written. On Unix (a POSIX
/// shell under tmux) the whole override is single-quoted, which the shell
/// takes literally, and each word is a TOML basic string with its
/// backslashes escaped for TOML. Windows PowerShell 5 drops double quotes
/// inside an argument it passes to a program, so there the override is
/// double-quoted, in which PowerShell reads a backslash as itself, and each
/// word is a TOML literal string, read as written. No word holds a quote, a
/// `$` or a backtick ([`UNSAFE`]).
fn codex_notify_flag(words: &[String]) -> String {
    let list: Vec<String> = if cfg!(windows) {
        words.iter().map(|w| format!("'{w}'")).collect()
    } else {
        words
            .iter()
            .map(|w| format!("\"{}\"", w.replace('\\', "\\\\")))
            .collect()
    };
    let toml = format!("notify=[{}]", list.join(","));
    if cfg!(windows) {
        format!("-c \"{toml}\"")
    } else {
        format!("-c '{toml}'")
    }
}

impl Hooks {
    /// Hooks for a node serving on `addr`. A node bound to every interface
    /// is reached on loopback, `[::1]` for an IPv6 bind. Refused when this
    /// binary's path or the folder can't be typed safely; the node then
    /// launches without hooks and says so in its launcher.
    pub fn new(exe: &Path, addr: SocketAddr, dir: PathBuf, codex_config: PathBuf) -> Result<Hooks> {
        let ip: IpAddr = match addr.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
            IpAddr::V6(ip) if ip.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
            ip => ip,
        };
        let hooks = Hooks {
            exe: exe
                .to_str()
                .context("this binary's path is not UTF-8")?
                .into(),
            url: format!("http://{}{PATH}", SocketAddr::new(ip, addr.port())),
            dir,
            codex_config,
        };
        for word in [&hooks.exe, &hooks.dir.to_string_lossy().into_owned()] {
            if word.contains(UNSAFE) {
                bail!("{word} holds a quote or $, which can't be typed safely");
            }
        }
        Ok(hooks)
    }

    fn url(&self, session: &str, kind: Kind) -> String {
        format!(
            "{}?session={}&event={}",
            self.url,
            encode(session),
            kind.word()
        )
    }

    /// Settles the hooks of a launch of `command` with these `flags`, before
    /// its session exists: `None` for a harness that takes none, else what
    /// [`Hooks::flags`] will add. Refused, with what to tell the operator,
    /// where installing them would override something of the operator's: their own
    /// `--settings` or `notify` in the flags, or a Codex `notify` that can't
    /// be passed on.
    pub fn plan(&self, command: &str, flags: &str) -> Result<Option<Plan>, String> {
        let Some(harness) = harness(command) else {
            return Ok(None);
        };
        let then = match harness {
            Harness::Claude if sets_claude_settings(flags) => {
                return Err(
                    "the flags name their own --settings, which Parvi's hooks would need".into(),
                );
            }
            Harness::Codex if sets_codex_notify(flags) => {
                return Err("the flags set Codex's notify, which Parvi's hook would need".into());
            }
            Harness::Claude => {
                std::fs::create_dir_all(&self.dir)
                    .map_err(|e| format!("creating {}: {e}", self.dir.display()))?;
                vec![]
            }
            Harness::Codex => {
                let then = codex_notify(&self.codex_config).map_err(|e| format!("{e:#}"))?;
                if then.iter().any(|w| w.contains(UNSAFE)) {
                    return Err(format!(
                        "the notify in {} holds a quote or $, so it can't be passed on",
                        self.codex_config.display()
                    ));
                }
                then
            }
        };
        Ok(Some(Plan { harness, then }))
    }

    /// The flags that install `plan`'s hooks in the session with this hub
    /// id, placed before the launcher's own. Claude Code's settings file is
    /// written here.
    pub fn flags(&self, plan: &Plan, session: &str) -> Result<String> {
        Ok(match plan.harness {
            Harness::Codex => {
                let mut words = vec![
                    self.exe.clone(),
                    VERB.into(),
                    self.url(session, Kind::Finished),
                ];
                if !plan.then.is_empty() {
                    words.push(THEN.into());
                    words.extend(plan.then.iter().cloned());
                }
                codex_notify_flag(&words)
            }
            Harness::Claude => {
                let hook = |kind: Kind| {
                    json!([{ "hooks": [{
                        "type": "command",
                        "command": format!("'{}' {VERB} '{}'", self.exe, self.url(session, kind)),
                        "timeout": CLAUDE_HOOK_SECONDS,
                    }]}])
                };
                let settings = json!({ "hooks": {
                    "Stop": hook(Kind::Finished),
                    "Notification": hook(Kind::NeedsInput),
                }});
                let path = self.dir.join(format!("claude-{}.json", encode(session)));
                let text = serde_json::to_string_pretty(&settings).expect("JSON") + "\n";
                write_atomically(&path, &text).context("writing Claude Code's hook settings")?;
                format!("--settings '{}'", path.display())
            }
        })
    }

    /// Forgets the settings files of sessions no longer listed, but not one
    /// just written: its session may not be listed yet.
    pub fn prune(&self, live: &[String]) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let keep: Vec<String> = live
            .iter()
            .map(|s| format!("claude-{}.json", encode(s)))
            .collect();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let fresh = entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|at| at.elapsed().unwrap_or_default() < JUST_WRITTEN);
            if name.starts_with("claude-")
                && name.ends_with(".json")
                && !keep.contains(&name)
                && !fresh
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Writes `path` whole or not at all: a new file under a name of its own,
/// renamed over the old.
fn write_atomically(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().context("the file has no folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = path.file_name().context("the file has no name")?;
    let temporary = dir.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    std::fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("parvi-hooks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hooks(dir: &Path) -> Hooks {
        Hooks::new(
            Path::new("/opt/parvi/parvi"),
            "0.0.0.0:7300".parse().unwrap(),
            dir.join("agent-hooks"),
            dir.join("codex/config.toml"),
        )
        .unwrap()
    }

    fn flags(h: &Hooks, command: &str, flags: &str, session: &str) -> Option<String> {
        let plan = h.plan(command, flags).unwrap()?;
        Some(h.flags(&plan, session).unwrap())
    }

    #[test]
    fn codex_is_given_a_notify_program_both_shells_read_alike() {
        let dir = scratch("codex");
        let h = hooks(&dir);
        let expected = if cfg!(windows) {
            "-c \"notify=['/opt/parvi/parvi','hook',\
             'http://127.0.0.1:7300/api/agent/event?session=%243%40alice&event=finished']\""
        } else {
            "-c 'notify=[\"/opt/parvi/parvi\",\"hook\",\
             \"http://127.0.0.1:7300/api/agent/event?session=%243%40alice&event=finished\"]'"
        };
        assert_eq!(flags(&h, "codex", "", "$3@alice").unwrap(), expected);
        assert_eq!(flags(&h, "bash", "", "$3"), None);
        assert_eq!(flags(&h, "", "", "$3"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_operators_own_codex_notify_runs_after_parvis_report() {
        let dir = scratch("compose");
        let h = hooks(&dir);
        std::fs::create_dir_all(dir.join("codex")).unwrap();
        std::fs::write(
            dir.join("codex/config.toml"),
            "model = \"o3\"\nnotify = [\"notify-send\", \"Codex\"]\n",
        )
        .unwrap();
        let notify = toml_of(&flags(&h, "codex", "", "$3").unwrap());
        assert_eq!(notify[3..], ["--then", "notify-send", "Codex"]);
        std::fs::write(dir.join("codex/config.toml"), "notify = [\"say '$x'\"]\n").unwrap();
        assert!(
            h.plan("codex", "")
                .unwrap_err()
                .contains("can't be passed on")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The notify words in a `-c notify=[…]` flag, as Codex reads them.
    fn toml_of(flag: &str) -> Vec<String> {
        let toml = flag
            .trim_start_matches("-c ")
            .trim_matches(['\'', '"'])
            .to_string();
        let doc: toml::Table = toml::from_str(&toml).unwrap();
        doc["notify"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap().to_string())
            .collect()
    }

    /// Typed into a POSIX shell, as tmux's session runs it, and read as
    /// TOML, as Codex reads its `-c`: every backslash arrives as written.
    #[cfg(unix)]
    #[test]
    fn backslashes_in_codex_notify_reach_it_through_the_shell_as_written() {
        let dir = scratch("backslash");
        let exe = r"/opt/a\\b\c/parvi";
        let h = Hooks::new(
            Path::new(exe),
            "127.0.0.1:7300".parse().unwrap(),
            dir.join("agent-hooks"),
            dir.join("codex/config.toml"),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("codex")).unwrap();
        std::fs::write(
            dir.join("codex/config.toml"),
            r"notify = ['\\srv\notify.exe', 'a\\\b\']",
        )
        .unwrap();
        let flag = flags(&h, "codex", "--full-auto", "$3").unwrap();
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("printf '%s\\0' {flag}"))
            .output()
            .unwrap();
        assert!(out.status.success());
        let argv: Vec<String> = String::from_utf8(out.stdout)
            .unwrap()
            .split_terminator('\0')
            .map(str::to_string)
            .collect();
        assert_eq!(argv[0], "-c");
        let toml: toml::Table = toml::from_str(&argv[1]).unwrap();
        let words: Vec<&str> = toml["notify"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap())
            .collect();
        assert_eq!(words[0], exe);
        assert_eq!(words[3..], ["--then", r"\\srv\notify.exe", r"a\\\b\"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn flags_that_would_override_the_hooks_install_none_and_say_why() {
        let dir = scratch("override");
        let h = hooks(&dir);
        for f in [
            "-c notify=['x']",
            "--config 'notify = [\"x\"]'",
            "--config=notify=['x']",
        ] {
            assert!(h.plan("codex", f).is_err(), "{f}");
        }
        assert!(
            h.plan("codex", "-c model_notify_level=1 --full-auto")
                .is_ok()
        );
        assert!(h.plan("claude", "--settings /tmp/mine.json").is_err());
        assert!(h.plan("claude", "--settings=/tmp/mine.json").is_err());
        assert!(h.plan("claude", "--model opus").is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn claude_reads_its_hooks_from_a_file_per_session_which_goes_with_it() {
        let dir = scratch("claude");
        let h = hooks(&dir);
        let flags = flags(&h, "/usr/bin/claude", "", "$7").unwrap();
        let path = dir.join("agent-hooks").join("claude-%247.json");
        assert_eq!(flags, format!("--settings '{}'", path.display()));
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            doc["hooks"]["Notification"][0]["hooks"][0]["command"],
            "'/opt/parvi/parvi' hook \
             'http://127.0.0.1:7300/api/agent/event?session=%247&event=needs-input'"
        );
        assert_eq!(
            doc["hooks"]["Stop"][0]["hooks"][0]["timeout"],
            CLAUDE_HOOK_SECONDS
        );
        h.prune(&[]);
        assert!(path.exists(), "just written");
        let old = std::time::SystemTime::now() - 2 * JUST_WRITTEN;
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(old).unwrap();
        h.prune(&["$7".into()]);
        assert!(path.exists(), "still listed");
        h.prune(&[]);
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_hook_reaches_a_node_bound_to_every_ipv6_address_on_ipv6_loopback() {
        let h = Hooks::new(
            Path::new("/opt/parvi"),
            "[::]:7300".parse().unwrap(),
            "/tmp".into(),
            "/tmp/config.toml".into(),
        )
        .unwrap();
        assert_eq!(h.url, "http://[::1]:7300/api/agent/event");
    }

    #[test]
    fn a_path_a_shell_would_expand_installs_nothing() {
        let addr = "127.0.0.1:7300".parse().unwrap();
        let codex = PathBuf::from("/tmp/config.toml");
        assert!(
            Hooks::new(
                Path::new("/home/o'neil/parvi"),
                addr,
                "/tmp".into(),
                codex.clone()
            )
            .is_err()
        );
        assert!(Hooks::new(Path::new("/opt/parvi"), addr, "/tmp/$x".into(), codex).is_err());
    }
}
