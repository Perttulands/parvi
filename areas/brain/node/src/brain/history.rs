//! The corpus as git knows it: what changed lately, when each page last
//! moved and who moved it, and the operator's own edits committed back
//! (CHROTE `library.go`). Everything runs the git binary inside the corpus
//! root, with paths relative to it, so a corpus that is a folder of a larger
//! checkout reads the same as one that is a checkout of its own.
//!
//! A refusal from git is not an empty history: every read carries what git
//! said beside what it could read, and the page itself is read off disk
//! whatever git says.

use parvi_brain_contract::Commit;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use axum::http::StatusCode;

use tokio::process::Command;

use super::refusal::Refusal;

/// How many commits a page's history carries.
pub const HISTORY_LIMIT: usize = 20;
/// How long one git invocation may take.
const GIT_TIMEOUT: Duration = Duration::from_secs(20);

/// git writes these ASCII separators for us, so a subject holding a tab or a
/// newline still parses.
const RECORD: char = '\x1e';
const FIELD: char = '\x1f';
const LOG_FORMAT: &str = "--format=\x1e%H\x1f%cI\x1f%an\x1f%s";

/// A commit and the corpus-relative files it touched.
#[derive(Debug, PartialEq)]
pub struct Change {
    pub commit: Commit,
    pub files: Vec<String>,
}

/// Runs git in `root` and returns its output, or what it said refusing.
pub async fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        // Paths as they are, not octal-quoted, so a page named in Finnish
        // is the page.
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(GIT_TIMEOUT, command.output()).await {
        Err(_) => return Err("git took too long".into()),
        Ok(Err(error)) => return Err(format!("running git: {error}")),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.trim();
    Err(match said.is_empty() {
        true => format!("git {} failed ({})", args[0], output.status),
        false => said.to_owned(),
    })
}

/// The newest `limit` commits under the root, newest first, each with the
/// files it touched.
pub async fn changes(root: &Path, limit: usize) -> (Vec<Change>, Option<String>) {
    let limit = limit.to_string();
    let args = ["log", "-n", &limit, LOG_FORMAT, "--name-only", "--relative"];
    match git(root, &args).await {
        Ok(out) => (parse_log(&out), None),
        Err(error) => (Vec::new(), Some(error)),
    }
}

/// The commits that touched the page at `path`, newest first, following it
/// across moves.
pub async fn page(root: &Path, path: &str) -> (Vec<Commit>, Option<String>) {
    let limit = HISTORY_LIMIT.to_string();
    let args = ["log", "--follow", "-n", &limit, LOG_FORMAT, "--", path];
    match git(root, &args).await {
        Ok(out) => (
            parse_log(&out).into_iter().map(|c| c.commit).collect(),
            None,
        ),
        Err(error) => (Vec::new(), Some(error)),
    }
}

/// The commit that last touched each file under `folder`.
pub async fn last_changes(root: &Path, folder: &str) -> (HashMap<String, Commit>, Option<String>) {
    let args = ["log", LOG_FORMAT, "--name-only", "--relative", "--", folder];
    let (log, error) = match git(root, &args).await {
        Ok(out) => (parse_log(&out), None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let mut last = HashMap::new();
    // Newest first: the first commit naming a file is its last change.
    for change in log {
        for file in change.files {
            last.entry(file).or_insert_with(|| change.commit.clone());
        }
    }
    (last, error)
}

/// How an edit is signed: `Name <email>`, or git's own identity for the
/// corpus when none is configured.
#[derive(Clone, Debug, Default)]
pub struct Author(Option<(String, String)>);

impl Author {
    /// Reads `Name <email>`, the form git itself writes; a form git would
    /// refuse is refused now rather than at the first save.
    pub fn parse(author: Option<&str>) -> anyhow::Result<Author> {
        let Some(author) = author.map(str::trim) else {
            return Ok(Author(None));
        };
        let parsed = author
            .strip_suffix('>')
            .and_then(|rest| rest.rsplit_once('<'))
            .map(|(name, email)| (name.trim(), email.trim()))
            .filter(|(name, email)| {
                !name.is_empty()
                    && !email.is_empty()
                    && !name.contains(['<', '>'])
                    && !email.contains(['<', '>', ' '])
            });
        match parsed {
            Some((name, email)) => Ok(Author(Some((name.into(), email.into())))),
            None => anyhow::bail!("the brain author {author:?} must read \"Name <email>\""),
        }
    }
}

/// Writes `text` to the page at `path` (corpus-relative; `file` is where it
/// is on disk) and commits it alone, as `author`, with `message`. A page
/// with an uncommitted change already is refused rather than swallowed into
/// the operator's commit. A save that changes nothing commits nothing. A
/// commit git refuses puts the page back as it was.
pub async fn save(
    root: &Path,
    file: &Path,
    path: &str,
    text: &str,
    message: &str,
    author: &Author,
) -> Result<Commit, Refusal> {
    let failed = |error: String| Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, error);
    let status = git(root, &["status", "--porcelain", "--", path])
        .await
        .map_err(failed)?;
    if !status.trim().is_empty() {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            format!(
                "{path} already has an uncommitted change in the corpus; \
                 commit or discard it before saving from here"
            ),
        ));
    }
    let before = tokio::fs::read(file)
        .await
        .map_err(|error| Refusal::new(StatusCode::FORBIDDEN, error.to_string()))?;
    tokio::fs::write(file, text)
        .await
        .map_err(|error| Refusal::new(StatusCode::FORBIDDEN, error.to_string()))?;
    let written = git(root, &["status", "--porcelain", "--", path])
        .await
        .map_err(failed)?;
    if !written.trim().is_empty()
        && let Err(error) = commit(root, path, message, author).await
    {
        // Clean before the write, so HEAD holds what was there.
        let _ = tokio::fs::write(file, &before).await;
        let _ = git(root, &["reset", "-q", "--", path]).await;
        return Err(failed(error));
    }
    let (history, error) = page(root, path).await;
    match (history.into_iter().next(), error) {
        (_, Some(error)) => Err(failed(error)),
        (commit, None) => Ok(commit.unwrap_or_default()),
    }
}

async fn commit(root: &Path, path: &str, message: &str, author: &Author) -> Result<(), String> {
    git(root, &["add", "--", path]).await?;
    let mut args: Vec<String> = Vec::new();
    if let Author(Some((name, email))) = author {
        args.extend([
            "-c".into(),
            format!("user.name={name}"),
            "-c".into(),
            format!("user.email={email}"),
        ]);
    }
    args.extend(["commit", "-q", "-m", message, "--", path].map(String::from));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    git(root, &args).await.map(drop)
}

/// The records of a `git log` run with `LOG_FORMAT`: with `--name-only`,
/// each with the files it touched.
fn parse_log(output: &str) -> Vec<Change> {
    output
        .split(RECORD)
        .filter_map(|record| {
            let mut lines = record.lines();
            let fields: Vec<&str> = lines.next()?.splitn(4, FIELD).collect();
            let [hash, time, author, message] = fields[..] else {
                return None;
            };
            Some(Change {
                commit: Commit {
                    hash: hash.into(),
                    time: time.into(),
                    author: author.into(),
                    message: message.into(),
                },
                files: lines
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_log_with_and_without_files() {
        let log = "\x1eaaa\x1f2026-11-01T10:00:00+02:00\x1fPerttu\x1fEdit a\tpage\n\n\
                   knowledge/a.md\nself/b.md\n\x1ebbb\x1f2026-10-01T10:00:00+02:00\x1fLibrarian\x1fFirst\n";
        let changes = parse_log(log);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].commit.message, "Edit a\tpage");
        assert_eq!(changes[0].files, ["knowledge/a.md", "self/b.md"]);
        assert_eq!(changes[1].commit.author, "Librarian");
        assert!(changes[1].files.is_empty());
        assert!(parse_log("").is_empty());
    }

    #[test]
    fn reads_an_author_as_git_writes_one() {
        let Author(Some((name, email))) =
            Author::parse(Some(" Ada Lovelace <ada@example.org> ")).unwrap()
        else {
            panic!("no author");
        };
        assert_eq!(
            (name.as_str(), email.as_str()),
            ("Ada Lovelace", "ada@example.org")
        );
        assert!(Author::parse(None).unwrap().0.is_none());
        for bad in ["Ada", "<ada@example.org>", "Ada <>", "Ada <a b>", "A<b <c>"] {
            assert!(Author::parse(Some(bad)).is_err(), "{bad}");
        }
    }
}
