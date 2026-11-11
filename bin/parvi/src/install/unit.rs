//! The systemd user unit that runs the node: its text, written by `parvi
//! install`, and what `parvi upgrade` reads back from it (which binary and
//! which address).

use std::path::{Path, PathBuf};

use parvi_core::Error;

/// What the unit says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// The installed `parvi`, which the unit runs as `parvi serve`.
    pub binary: PathBuf,
    /// `addr:port`, given to `serve --listen`.
    pub listen: String,
    /// `serve --config`, when not the node's own config folder.
    pub config: Option<PathBuf>,
    /// `NAME=value` pairs the node starts with (PATH, PARVI_TMUX).
    pub environment: Vec<(String, String)>,
}

/// The line that keeps sessions through a stop or restart. A unit without
/// it would take a tmux server the node started down with the node.
pub const KEEPS_SESSIONS: &str = "KillMode=process";

impl Unit {
    /// The unit file's text.
    pub fn render(&self) -> String {
        let mut exec = vec![
            quote(&self.binary.to_string_lossy(), Line::Exec),
            "serve".into(),
            "--listen".into(),
            quote(&self.listen, Line::Exec),
        ];
        if let Some(config) = &self.config {
            exec.push("--config".into());
            exec.push(quote(&config.to_string_lossy(), Line::Exec));
        }
        let environment: String = self
            .environment
            .iter()
            .map(|(name, value)| {
                format!(
                    "Environment={}\n",
                    quote(&format!("{name}={value}"), Line::Env)
                )
            })
            .collect();
        format!(
            "# Written by parvi install; parvi upgrade reads ExecStart.
[Unit]
Description=Parvi node
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=%h
{environment}ExecStart={exec}
# Terminal sessions belong to tmux, not to the node. Stopping or restarting
# the unit signals only the node, so a tmux server it started lives on with
# every session.
{KEEPS_SESSIONS}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
",
            exec = exec.join(" "),
        )
    }

    /// Reads back what [`Unit::render`] wrote, or a unit edited by hand
    /// that still runs `parvi serve --listen`. Refuses a unit that would
    /// stop sessions with the node.
    pub fn parse(text: &str, path: &Path) -> Result<Unit, Error> {
        let refused = |why: &str| {
            Error::new(
                "unit_unreadable",
                format!("{}: {why}; parvi install writes one", path.display()),
            )
        };
        let mut exec = None;
        let mut environment = Vec::new();
        let mut keeps_sessions = false;
        for line in text.lines().map(str::trim) {
            if let Some(command) = line.strip_prefix("ExecStart=") {
                exec = Some(
                    words(command, Line::Exec)
                        .ok_or_else(|| refused("ExecStart has an open quote"))?,
                );
            } else if let Some(assignment) = line.strip_prefix("Environment=") {
                for word in words(assignment, Line::Env).ok_or_else(|| refused("an open quote"))? {
                    if let Some((name, value)) = word.split_once('=') {
                        environment.push((name.to_owned(), value.to_owned()));
                    }
                }
            } else if line == KEEPS_SESSIONS {
                keeps_sessions = true;
            }
        }
        let exec = exec.ok_or_else(|| refused("no ExecStart"))?;
        let (binary, rest) = exec
            .split_first()
            .ok_or_else(|| refused("ExecStart is empty"))?;
        if rest.first().map(String::as_str) != Some("serve") {
            return Err(refused("ExecStart does not run parvi serve"));
        }
        let value = |flag: &str| {
            rest.iter()
                .position(|word| word == flag)
                .and_then(|at| rest.get(at + 1))
                .cloned()
        };
        let listen = value("--listen").ok_or_else(|| refused("ExecStart has no --listen"))?;
        if !keeps_sessions {
            return Err(Error::new(
                "unit_kills_sessions",
                format!(
                    "{} has no {KEEPS_SESSIONS}, so restarting it would stop every session \
                     the node started; run parvi install to rewrite it",
                    path.display()
                ),
            ));
        }
        Ok(Unit {
            binary: PathBuf::from(binary),
            listen,
            config: value("--config").map(PathBuf::from),
            environment,
        })
    }
}

/// Where a word goes: in ExecStart `$` names a variable, in Environment
/// it is itself.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Line {
    Exec,
    Env,
}

/// One word as systemd reads it: `%` is a specifier, so it is doubled, and
/// so is `$` in ExecStart; a word with a space or a quote goes in double
/// quotes, with `\` and `"` escaped.
fn quote(word: &str, line: Line) -> String {
    let mut escaped = word.replace('%', "%%");
    if line == Line::Exec {
        escaped = escaped.replace('$', "$$");
    }
    let plain = !escaped.is_empty()
        && escaped
            .chars()
            .all(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '\\' | ';'));
    if plain {
        return escaped;
    }
    let mut quoted = String::from('"');
    for c in escaped.chars() {
        match c {
            '"' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '\n' => quoted.push_str("\\n"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// The words of a line [`quote`] wrote. `None` for an unclosed quote.
fn words(text: &str, line: Line) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else {
            return Some(words);
        };
        let mut word = String::new();
        if first == '"' {
            chars.next();
            loop {
                match chars.next()? {
                    '"' => break,
                    '\\' => match chars.next()? {
                        'n' => word.push('\n'),
                        c => word.push(c),
                    },
                    c => word.push(c),
                }
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                word.push(c);
            }
        }
        let word = word.replace("%%", "%");
        words.push(match line {
            Line::Exec => word.replace("$$", "$"),
            Line::Env => word,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit() -> Unit {
        Unit {
            binary: "/home/op/.local/bin/parvi".into(),
            listen: "127.0.0.1:7320".into(),
            config: Some("/home/op/parvi next/config.toml".into()),
            environment: vec![
                (
                    "PATH".into(),
                    "/home/op/.local/bin:/mnt/c/Program Files/x:/usr/bin".into(),
                ),
                ("PARVI_TMUX".into(), "/opt/$tmux 100%/tmux".into()),
            ],
        }
    }

    #[test]
    fn the_unit_text_is_the_golden_one() {
        let golden = r#"# Written by parvi install; parvi upgrade reads ExecStart.
[Unit]
Description=Parvi node
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=%h
Environment="PATH=/home/op/.local/bin:/mnt/c/Program Files/x:/usr/bin"
Environment="PARVI_TMUX=/opt/$tmux 100%%/tmux"
ExecStart=/home/op/.local/bin/parvi serve --listen 127.0.0.1:7320 --config "/home/op/parvi next/config.toml"
# Terminal sessions belong to tmux, not to the node. Stopping or restarting
# the unit signals only the node, so a tmux server it started lives on with
# every session.
KillMode=process
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
"#;
        assert_eq!(unit().render(), golden);
    }

    #[test]
    fn upgrade_reads_back_what_install_wrote() {
        let path = Path::new("/u/parvi.service");
        assert_eq!(Unit::parse(&unit().render(), path), Ok(unit()));
        let odd = Unit {
            binary: "/a b/\"q\"/$HOME/back\\slash/parvi".into(),
            config: None,
            ..unit()
        };
        assert_eq!(Unit::parse(&odd.render(), path), Ok(odd));
    }

    #[test]
    fn a_unit_that_would_stop_sessions_is_refused() {
        let text = unit()
            .render()
            .replace(KEEPS_SESSIONS, "KillMode=control-group");
        let error = Unit::parse(&text, Path::new("/u/parvi.service")).unwrap_err();
        assert_eq!(error.code, "unit_kills_sessions");
        let error = Unit::parse("[Service]\nExecStart=/bin/true\n", Path::new("/u/x")).unwrap_err();
        assert_eq!(error.code, "unit_unreadable");
    }
}
