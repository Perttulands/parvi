//! The command-line tools Agents reports: the harnesses and `gh`. For each,
//! where it is, its version, whether it is signed in and which of its
//! settings files exist.
//!
//! Credential presence is checked without opening files or showing values.
//! Presence alone cannot establish whether a tool is signed in.

use std::path::{Path, PathBuf};

use super::{Home, run};

pub struct Tool {
    pub id: &'static str,
    pub name: &'static str,
    pub command: &'static str,
    /// The file whose presence means signed in, under the home folder.
    pub credentials: &'static str,
    /// Settings files under the home folder, as the operator names them.
    pub settings: &'static [&'static str],
    /// The skills folder it reads, by its id in [`super::skills::FOLDERS`].
    pub skills: Option<&'static str>,
}

pub const TOOLS: &[Tool] = &[
    Tool {
        id: "claude",
        name: "Claude Code",
        command: "claude",
        credentials: ".claude/.credentials.json",
        settings: &[".claude/settings.json", ".claude/CLAUDE.md"],
        skills: Some("claude"),
    },
    Tool {
        id: "codex",
        name: "Codex",
        command: "codex",
        credentials: ".codex/auth.json",
        settings: &[".codex/config.toml", ".codex/AGENTS.md"],
        skills: Some("agents"),
    },
    Tool {
        id: "pi",
        name: "Pi",
        command: "pi",
        credentials: ".pi/agent/auth.json",
        settings: &[".pi/agent/settings.json"],
        skills: Some("agents"),
    },
    Tool {
        id: "gh",
        name: "GitHub CLI",
        command: "gh",
        credentials: ".config/gh/hosts.yml",
        settings: &[".config/gh/config.yml"],
        skills: None,
    },
];

pub struct Found {
    pub tool: &'static Tool,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub credentials_present: bool,
    /// Each settings file and whether it exists.
    pub settings: Vec<(&'static str, bool)>,
}

/// Looks the tool up in `home`, asking it for its version.
pub async fn find(home: &Home, tool: &'static Tool) -> Found {
    let path = home.which(tool.command);
    let version = match &path {
        Some(path) => version(home, path).await,
        None => None,
    };
    Found {
        tool,
        version,
        path,
        credentials_present: credentials_present(home, tool, |name| std::env::var_os(name)),
        settings: tool
            .settings
            .iter()
            .map(|&file| (file, under(home, file).is_file()))
            .collect(),
    }
}

fn credentials_present(
    home: &Home,
    tool: &Tool,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> bool {
    if tool.id == "gh" {
        if [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
        ]
        .iter()
        .any(|name| env(name).is_some())
        {
            return true;
        }
        let config = env("GH_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env("XDG_CONFIG_HOME")
                    .filter(|v| !v.is_empty())
                    .map(|v| PathBuf::from(v).join("gh"))
            })
            .unwrap_or_else(|| home.dir.join(".config/gh"));
        return present(&config.join("hosts.yml"));
    }
    let (tokens, config_var, file): (&[&str], &str, &str) = match tool.id {
        "claude" => (
            &["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"],
            "CLAUDE_CONFIG_DIR",
            ".credentials.json",
        ),
        "codex" => (&["OPENAI_API_KEY"], "CODEX_HOME", "auth.json"),
        "pi" => (
            &["ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
            "PI_CODING_AGENT_DIR",
            "auth.json",
        ),
        _ => (&[], "", ""),
    };
    if tokens.iter().any(|name| env(name).is_some()) {
        return true;
    }
    let credentials = env(config_var)
        .filter(|v| !v.is_empty())
        .map(|dir| PathBuf::from(dir).join(file))
        .unwrap_or_else(|| under(home, tool.credentials));
    present(&credentials)
}

fn under(home: &Home, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(home.dir.clone(), |dir, part| dir.join(part))
}

/// Exists and is not empty, from its metadata alone.
fn present(path: &Path) -> bool {
    path.metadata().is_ok_and(|m| m.is_file() && m.len() > 0)
}

async fn version(home: &Home, path: &Path) -> Option<String> {
    let mut command = run::command(home, path);
    command.arg("--version");
    parse_version(&run::stdout(command).await?)
}

/// The first word that looks like a version: `codex-cli 0.166.0`,
/// `2.1.318 (Claude Code)`, `gh version 2.103.3 (2026-10-26)`, `v1.5.0`.
fn parse_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .map(|word| {
            word.trim_start_matches('v')
                .trim_end_matches([',', ';', ')'])
        })
        .find(|word| {
            word.starts_with(|c: char| c.is_ascii_digit())
                && word.contains('.')
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
        })
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_checks_token_presence_and_config_override_without_reading_files() {
        let dir = std::env::temp_dir().join(format!("parvi-82-gh-{}", std::process::id()));
        let config = dir.join("custom");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("hosts.yml"), "secret").unwrap();
        let home = Home {
            dir: dir.clone(),
            path: vec![],
        };
        let gh = &TOOLS[3];
        assert!(credentials_present(&home, gh, |key| (key == "GH_TOKEN")
            .then(|| "secret".into())));
        assert!(credentials_present(&home, gh, |key| (key
            == "GH_CONFIG_DIR")
            .then(|| config.clone().into_os_string())));
        assert!(!credentials_present(&home, gh, |_| None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_versions_the_way_each_tool_prints_them() {
        for (output, version) in [
            ("2.1.318 (Claude Code)\n", Some("2.1.318")),
            ("codex-cli 0.166.0\n", Some("0.166.0")),
            ("1.5.0\n", Some("1.5.0")),
            (
                "gh version 2.103.3 (2026-10-26)\nhttps://github.com/cli/cli/releases/tag/v2.103.3\n",
                Some("2.103.3"),
            ),
            ("tool v0.9.1-beta.2\n", Some("0.9.1-beta.2")),
            ("no version here\n", None),
        ] {
            assert_eq!(parse_version(output).as_deref(), version, "{output:?}");
        }
    }
}
