//! Which projects the Work module reads, and with which tracker: a Beads
//! store in a project folder, or a GitHub repository and where it is checked
//! out on this node. See `docs/work.md`, Configuration.

use anyhow::{Context, Result, bail};

/// The projects to read, in the order the tab offers them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub projects: Vec<Project>,
}

/// One project and its tracker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Project {
    GitHub(Repo),
    Beads(Store),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// `owner/name`, as GitHub spells it.
    pub name: String,
    /// The folder a session for one of its issues starts in, if this node
    /// has a checkout: absolute or starting with `~`.
    pub checkout: Option<String>,
}

/// A Beads store: the project folder whose `.beads` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    /// Absolute, as written; references name the store by it.
    pub folder: String,
}

impl Repo {
    pub fn owner_and_name(&self) -> (&str, &str) {
        self.name.split_once('/').expect("checked when parsed")
    }
}

impl Config {
    /// Parses lines of `beads <folder>` and `github <owner>/<name>
    /// [checkout folder]`. `#` starts a comment line.
    pub fn parse(text: &str) -> Result<Config> {
        let mut projects: Vec<Project> = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let at = || format!("line {}", number + 1);
            let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let rest = rest.trim();
            let project = match key {
                "beads" => {
                    let folder = trim_separators(rest);
                    if !parvi_work_contract::is_absolute(folder) {
                        bail!(
                            "{}: a Beads store is named by its absolute project folder",
                            at()
                        );
                    }
                    Project::Beads(Store {
                        folder: folder.to_owned(),
                    })
                }
                "github" => Project::GitHub(repo(rest).with_context(at)?),
                _ => bail!(
                    "{}: unknown setting {key:?}; projects are beads or github lines",
                    at()
                ),
            };
            if let Some(twice) = projects.iter().find(|p| p.same(&project)) {
                bail!("{}: {} is named twice", at(), twice.name());
            }
            projects.push(project);
        }
        Ok(Config { projects })
    }
}

/// The folder without trailing separators, except a root's own: `/`,
/// `C:\\` and `\\\\` stay as they are.
fn trim_separators(folder: &str) -> &str {
    let b = folder.as_bytes();
    let root = if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        3
    } else if folder.starts_with("\\\\") {
        2
    } else {
        1
    };
    let mut end = folder.len();
    while end > root && matches!(b[end - 1], b'/' | b'\\') {
        end -= 1;
    }
    &folder[..end]
}

impl Project {
    /// The repository or the store's folder.
    pub fn name(&self) -> &str {
        match self {
            Project::GitHub(repo) => &repo.name,
            Project::Beads(store) => &store.folder,
        }
    }

    fn same(&self, other: &Project) -> bool {
        match (self, other) {
            (Project::GitHub(a), Project::GitHub(b)) => a.name.eq_ignore_ascii_case(&b.name),
            (Project::Beads(a), Project::Beads(b)) => a.folder == b.folder,
            _ => false,
        }
    }
}

fn repo(rest: &str) -> Result<Repo> {
    let name = rest.split_whitespace().next().unwrap_or("");
    if !is_repo_name(name) {
        bail!("a repository is <owner>/<name>");
    }
    let checkout = rest[name.len()..].trim();
    let checkout = (!checkout.is_empty()).then(|| checkout.to_owned());
    if let Some(folder) = &checkout
        && !(folder == "~"
            || folder.starts_with("~/")
            || folder.starts_with('/')
            || is_windows_absolute(folder)
            || folder.starts_with("~\\"))
    {
        bail!("a checkout is absolute or starts with ~");
    }
    Ok(Repo {
        name: name.to_owned(),
        checkout,
    })
}

fn is_windows_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/'))
        || path.starts_with("\\\\")
}

fn is_repo_name(name: &str) -> bool {
    matches!(name.split_once('/'), Some((owner, repo)) if parvi_work_contract::is_repo_part(owner) && parvi_work_contract::is_repo_part(repo))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github(name: &str, checkout: Option<&str>) -> Project {
        Project::GitHub(Repo {
            name: name.into(),
            checkout: checkout.map(Into::into),
        })
    }

    #[test]
    fn parses_stores_and_repositories_in_order() {
        let config = Config::parse(
            "# mine\nbeads /srv/example-project\ngithub example-owner/parvi ~/parvi\n\ngithub cli/cli\nbeads /srv/my project/\n",
        )
        .unwrap();
        assert_eq!(
            config.projects,
            vec![
                Project::Beads(Store {
                    folder: "/srv/example-project".into()
                }),
                github("example-owner/parvi", Some("~/parvi")),
                github("cli/cli", None),
                Project::Beads(Store {
                    folder: "/srv/my project".into()
                }),
            ]
        );
    }

    #[test]
    fn accepts_paths_with_spaces_and_windows_roots() {
        let c = Config::parse(
            "github a/b /srv/my checkout\ngithub c/d C:\\work\\my checkout\nbeads C:\\work\\x",
        )
        .unwrap();
        assert_eq!(c.projects[0], github("a/b", Some("/srv/my checkout")));
        assert_eq!(c.projects[1], github("c/d", Some("C:\\work\\my checkout")));
        assert_eq!(c.projects[2].name(), "C:\\work\\x");
        assert!(Config::parse("github a/b C:relative").is_err());
    }

    #[test]
    fn keeps_a_roots_separator_and_drops_the_rest() {
        for (line, folder) in [
            ("beads /", "/"),
            ("beads //", "/"),
            ("beads C:\\", "C:\\"),
            ("beads C:/", "C:/"),
            ("beads C:\\work\\", "C:\\work"),
            ("beads /srv/x//", "/srv/x"),
            ("beads \\\\host\\share\\", "\\\\host\\share"),
        ] {
            let config = Config::parse(line).unwrap();
            assert_eq!(config.projects[0].name(), folder, "{line}");
        }
    }

    #[test]
    fn rejects_what_would_reach_gh_or_bd_wrongly() {
        for bad in [
            "github parvi",
            "github /parvi",
            "github a/b/c",
            "github -F/x",
            "github a/b relative/path",
            "github a/b\ngithub A/B",
            "repo a/b",
            "github a/$(rm)",
            "beads",
            "beads ~/x",
            "beads relative",
            "beads /srv/x\nbeads /srv/x/",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
    }
}
