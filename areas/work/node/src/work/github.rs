//! Reads one repository through `gh api graphql`. gh holds the token and
//! Parvi never sees it (ARCHITECTURE.md: Work).

use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use super::config::Repo;
use super::github_item;
use super::item::Item;
use super::model::Work;
use super::parse::parse;
use super::tree;

/// The one query, for every repository.
const QUERY: &str = include_str!("query.graphql");

/// How long one read may take before it counts as failed.
const TIMEOUT: Duration = Duration::from_secs(30);

/// The gh program: `gh` on the path, or another for tests.
#[derive(Debug, Clone)]
pub struct Gh {
    program: String,
}

impl Default for Gh {
    fn default() -> Gh {
        Gh::new("gh")
    }
}

impl Gh {
    pub fn new(program: impl Into<String>) -> Gh {
        Gh {
            program: program.into(),
        }
    }

    /// The repository's issues and pull requests, or why they couldn't be
    /// read: GitHub's message, or gh's when GitHub never answered.
    pub async fn read(&self, repo: &Repo) -> Result<Work> {
        let mut cursors: [Option<String>; 4] = Default::default();
        let mut active = [true; 4];
        let mut result: Option<Work> = None;
        loop {
            let json = self.page(repo, &cursors).await?;
            let page = parse(&json)?;
            let value: serde_json::Value = serde_json::from_str(&json)?;
            let repository = &value["data"]["repository"];
            if let Some(work) = &mut result {
                work.issues
                    .extend(page.issues.into_iter().filter(|i| match i.state {
                        super::model::IssueState::Open => active[0],
                        super::model::IssueState::Closed => active[1],
                    }));
                work.pulls
                    .extend(page.pulls.into_iter().filter(|p| match p.state {
                        super::model::PullState::Open | super::model::PullState::Draft => active[2],
                        _ => active[3],
                    }));
            } else {
                result = Some(page);
            }
            for (i, key) in ["openIssues", "closedIssues", "openPulls", "closedPulls"]
                .iter()
                .enumerate()
            {
                if !active[i] {
                    continue;
                }
                let info = &repository[key]["pageInfo"];
                if info["hasNextPage"].as_bool().context("missing pageInfo")? {
                    let cursor = info["endCursor"].as_str().context("missing page cursor")?;
                    if cursors[i].as_deref() == Some(cursor) {
                        bail!("GitHub repeated a page cursor");
                    }
                    cursors[i] = Some(cursor.to_owned());
                } else {
                    active[i] = false;
                }
            }
            if !active.iter().any(|a| *a) {
                break;
            }
        }
        let mut work = result.context("GitHub sent no pages")?;
        work.issues
            .sort_by_key(|i| i.state == super::model::IssueState::Closed);
        work.pulls.sort_by_key(|p| {
            !matches!(
                p.state,
                super::model::PullState::Open | super::model::PullState::Draft
            )
        });
        Ok(work)
    }

    /// One issue with its family, read now.
    pub async fn item(&self, owner: &str, repo: &str, number: u64) -> Result<Item> {
        let json = self
            .run([
                format!("owner={owner}"),
                format!("name={repo}"),
                format!("number={number}"),
                format!("query={}", github_item::QUERY),
            ])
            .await?;
        github_item::parse(&json, owner, repo)
    }

    async fn page(&self, repo: &Repo, cursors: &[Option<String>; 4]) -> Result<String> {
        let (owner, name) = repo.owner_and_name();
        let mut fields = vec![
            format!("owner={owner}"),
            format!("name={name}"),
            format!("query={QUERY}"),
        ];
        for (key, cursor) in ["oi", "ci", "op", "cp"].iter().zip(cursors) {
            if let Some(cursor) = cursor {
                fields.push(format!("{key}={cursor}"));
            }
        }
        self.run(fields).await
    }

    /// Runs `gh api graphql` with `key=value` fields: `-F` for owner, name
    /// and number, so GitHub types them, `-f` for the rest.
    async fn run(&self, fields: impl IntoIterator<Item = String>) -> Result<String> {
        let mut command = Command::new(&self.program);
        command.args(["api", "graphql"]);
        for field in fields {
            let typed = ["owner=", "name=", "number="]
                .iter()
                .any(|key| field.starts_with(key));
            command.arg(if typed { "-F" } else { "-f" }).arg(field);
        }
        command.stdin(std::process::Stdio::null());
        let output = tokio::time::timeout(TIMEOUT, tree::output(&mut command))
            .await
            .context("gh took too long")?
            .with_context(|| format!("running {}", self.program))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim_start().starts_with('{') {
            return Ok(stdout.into_owned());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        match stderr.lines().map(str::trim).find(|line| !line.is_empty()) {
            Some(line) => bail!("{}", line.strip_prefix("gh: ").unwrap_or(line)),
            None => bail!("gh failed without saying why ({})", output.status),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_all_connections_until_their_cursors_end() {
        let dir = std::env::temp_dir().join(format!("parvi-67-pages-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut first: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/work/parvi.json")).unwrap();
        let mut second = first.clone();
        for key in ["openIssues", "closedIssues", "openPulls", "closedPulls"] {
            first["data"]["repository"][key]["pageInfo"] =
                serde_json::json!({"hasNextPage": key == "closedIssues", "endCursor": "next"});
            second["data"]["repository"][key]["pageInfo"] =
                serde_json::json!({"hasNextPage": false, "endCursor": "end"});
        }
        second["data"]["repository"]["closedIssues"]["nodes"][0]["number"] = 999999.into();
        std::fs::write(dir.join("first"), first.to_string()).unwrap();
        std::fs::write(dir.join("second"), second.to_string()).unwrap();
        let script = dir.join("gh");
        crate::test_support::write_script(
            &script,
            &format!(
                "#!/bin/sh\ncase \"$*\" in *ci=next*) cat '{}';; *) cat '{}';; esac\n",
                dir.join("second").display(),
                dir.join("first").display()
            ),
        );
        let work = Gh::new(script.to_str().unwrap())
            .read(&Repo {
                name: "a/b".into(),
                checkout: None,
            })
            .await
            .unwrap();
        assert_eq!(work.issues.len(), 15 + 20 + 20);
        assert!(work.issues.iter().any(|i| i.number == 999999));
        assert_eq!(work.pulls.len(), 20);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
