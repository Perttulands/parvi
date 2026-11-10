//! Reads GitHub's GraphQL answer to `query.graphql` into [`Work`].

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::model::{Checks, Issue, IssueState, Pull, PullState, Work};

/// Parses `gh api graphql`'s output for one repository. GraphQL errors,
/// such as a repository that doesn't exist, are errors carrying GitHub's
/// message.
pub fn parse(json: &str) -> Result<Work> {
    let answer: Answer = serde_json::from_str(json).context("GitHub's answer is not JSON")?;
    if !answer.errors.is_empty() {
        let messages: Vec<_> = answer.errors.into_iter().map(|e| e.message).collect();
        bail!("{}", messages.join("; "));
    }
    let Some(repo) = answer.data.and_then(|d| d.repository) else {
        bail!("GitHub sent no repository");
    };
    Ok(Work {
        url: repo.url,
        open_issues: repo.open_issues.total_count,
        closed_issues: repo.closed_issues.total_count,
        open_pulls: repo.open_pulls.total_count,
        closed_pulls: repo.closed_pulls.total_count,
        issues: repo
            .open_issues
            .nodes
            .into_iter()
            .chain(repo.closed_issues.nodes)
            .map(issue)
            .collect(),
        pulls: repo
            .open_pulls
            .nodes
            .into_iter()
            .chain(repo.closed_pulls.nodes)
            .map(pull)
            .collect(),
    })
}

fn issue(raw: RawIssue) -> Issue {
    Issue {
        number: raw.number,
        title: raw.title,
        url: raw.url,
        state: if raw.state == "CLOSED" {
            IssueState::Closed
        } else {
            IssueState::Open
        },
        author: raw.author.map(|a| a.login),
        created: raw.created_at,
        updated: raw.updated_at,
        body: raw.body,
        labels: names(raw.labels),
        assignees: logins(raw.assignees),
        branches: raw
            .linked_branches
            .nodes
            .into_iter()
            .filter_map(|b| b.r#ref.map(|r| r.name))
            .collect(),
        pulls: numbers(raw.closed_by_pull_requests_references),
    }
}

fn pull(raw: RawPull) -> Pull {
    let state = match raw.state.as_str() {
        "MERGED" => PullState::Merged,
        "CLOSED" => PullState::Closed,
        _ if raw.is_draft => PullState::Draft,
        _ => PullState::Open,
    };
    let checks = raw
        .commits
        .nodes
        .into_iter()
        .next()
        .and_then(|c| c.commit.status_check_rollup)
        .map(|rollup| {
            let contexts = rollup.contexts;
            count(
                contexts
                    .check_run_counts_by_state
                    .into_iter()
                    .chain(contexts.status_context_counts_by_state),
            )
        })
        .filter(|c| c.passed + c.failed + c.pending > 0);
    Pull {
        number: raw.number,
        title: raw.title,
        url: raw.url,
        state,
        author: raw.author.map(|a| a.login),
        created: raw.created_at,
        updated: raw.updated_at,
        body: raw.body,
        head: raw.head_ref_name,
        base: raw.base_ref_name,
        additions: raw.additions,
        deletions: raw.deletions,
        labels: names(raw.labels),
        assignees: logins(raw.assignees),
        closes: numbers(raw.closing_issues_references),
        checks,
    }
}

/// Counts GitHub's tallies of check runs and commit statuses by state. A
/// run passes when it ended as success, neutral or skipped, as GitHub's own
/// checks box counts it; one still queued, waiting or running is pending;
/// any other ending, such as cancelled or timed out, failed.
fn count(tallies: impl IntoIterator<Item = Tally>) -> Checks {
    let mut checks = Checks::default();
    for tally in tallies {
        let bucket = match tally.state.as_str() {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" | "COMPLETED" => &mut checks.passed,
            "PENDING" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "EXPECTED" => &mut checks.pending,
            _ => &mut checks.failed,
        };
        *bucket += tally.count;
    }
    checks
}

fn names(labels: Nodes<Named>) -> Vec<String> {
    labels.nodes.into_iter().map(|l| l.name).collect()
}

fn logins(users: Nodes<Login>) -> Vec<String> {
    users.nodes.into_iter().map(|u| u.login).collect()
}

fn numbers(items: Nodes<Numbered>) -> Vec<u32> {
    items.nodes.into_iter().map(|n| n.number).collect()
}

#[derive(Deserialize)]
struct Answer {
    data: Option<Data>,
    #[serde(default)]
    errors: Vec<Error>,
}

#[derive(Deserialize)]
struct Error {
    message: String,
}

#[derive(Deserialize)]
struct Data {
    repository: Option<Repository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    url: String,
    open_issues: Page<RawIssue>,
    closed_issues: Page<RawIssue>,
    open_pulls: Page<RawPull>,
    closed_pulls: Page<RawPull>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    total_count: u32,
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawIssue {
    number: u32,
    title: String,
    url: String,
    state: String,
    created_at: String,
    updated_at: String,
    body: String,
    author: Option<Login>,
    labels: Nodes<Named>,
    assignees: Nodes<Login>,
    linked_branches: Nodes<LinkedBranch>,
    closed_by_pull_requests_references: Nodes<Numbered>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPull {
    number: u32,
    title: String,
    url: String,
    state: String,
    is_draft: bool,
    created_at: String,
    updated_at: String,
    body: String,
    author: Option<Login>,
    head_ref_name: String,
    base_ref_name: String,
    additions: u32,
    deletions: u32,
    labels: Nodes<Named>,
    assignees: Nodes<Login>,
    closing_issues_references: Nodes<Numbered>,
    commits: Nodes<CommitNode>,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct Numbered {
    number: u32,
}

#[derive(Deserialize)]
struct LinkedBranch {
    r#ref: Option<Named>,
}

#[derive(Deserialize)]
struct CommitNode {
    commit: Commit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct Rollup {
    contexts: Tallies,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tallies {
    check_run_counts_by_state: Vec<Tally>,
    status_context_counts_by_state: Vec<Tally>,
}

#[derive(Deserialize)]
struct Tally {
    state: String,
    count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARVI: &str = include_str!("../../tests/fixtures/work/parvi.json");
    const CHECKS: &str = include_str!("../../tests/fixtures/work/checks.json");
    const MISSING: &str = include_str!("../../tests/fixtures/work/missing.json");

    #[test]
    fn reads_parvis_issues_and_pull_requests() {
        let work = parse(PARVI).unwrap();
        assert_eq!(work.url, "https://github.com/example-owner/parvi");
        assert_eq!(
            (work.open_issues, work.closed_issues, work.open_pulls),
            (15, 28, 0)
        );
        assert_eq!(work.closed_pulls, 27);
        // GitHub's totals; the closed lists are cut to the latest 20.
        assert_eq!((work.issues.len(), work.pulls.len()), (15 + 20, 20));
        let first = &work.issues[0];
        assert_eq!(first.number, 70);
        assert_eq!(first.state, IssueState::Open);
        assert_eq!(first.author.as_deref(), Some("example-owner"));
        assert!(first.body.starts_with("## Goal\n"));
        assert!(
            work.issues[..15]
                .iter()
                .all(|i| i.state == IssueState::Open)
        );
        assert!(
            work.issues[15..]
                .iter()
                .all(|i| i.state == IssueState::Closed)
        );
    }

    #[test]
    fn links_issues_and_the_pull_requests_that_close_them() {
        let work = parse(PARVI).unwrap();
        let issue = work.issues.iter().find(|i| i.number == 51).unwrap();
        assert_eq!(issue.pulls, vec![58]);
        let pull = work.pulls.iter().find(|p| p.number == 58).unwrap();
        assert_eq!(pull.state, PullState::Merged);
        assert!(pull.closes.contains(&51), "{:?}", pull.closes);
        assert_eq!(pull.base, "main");
        assert!(pull.additions > 0);
    }

    #[test]
    fn counts_a_commits_checks_and_leaves_them_out_when_there_are_none() {
        let work = parse(PARVI).unwrap();
        let checked = work.pulls.iter().find(|p| p.number == 45).unwrap();
        assert_eq!(
            checked.checks,
            Some(Checks {
                passed: 1,
                failed: 0,
                pending: 0
            })
        );
        let unchecked = work.pulls.iter().find(|p| p.number == 61).unwrap();
        assert_eq!(unchecked.checks, None);
    }

    #[test]
    fn counts_failed_pending_and_status_contexts() {
        let work = parse(CHECKS).unwrap();
        let checks = |passed, failed, pending| {
            Some(Checks {
                passed,
                failed,
                pending,
            })
        };
        let got: Vec<_> = work
            .pulls
            .iter()
            .map(|p| (p.number, p.state, p.checks))
            .collect();
        assert_eq!(
            got,
            vec![
                // Skipped runs pass, as GitHub's checks box counts them.
                (14921, PullState::Draft, checks(24, 1, 0)),
                (14845, PullState::Open, checks(11, 0, 0)),
                (10423, PullState::Open, None),
                // Commit statuses.
                (142515, PullState::Open, checks(12, 1, 3)),
                // Cancelled runs failed.
                (164759, PullState::Open, checks(3, 11, 0)),
                // Runs in progress.
                (163571, PullState::Open, checks(6, 0, 7)),
                (13556, PullState::Closed, None),
            ]
        );
    }

    #[test]
    fn a_missing_repository_is_githubs_message() {
        let error = parse(MISSING).unwrap_err().to_string();
        assert_eq!(
            error,
            "Could not resolve to a Repository with the name 'example-owner/no-such-repo-67'."
        );
    }

    #[test]
    fn rejects_what_is_not_githubs_answer() {
        assert!(parse("").is_err());
        assert!(parse("{\"data\":{\"repository\":null}}").is_err());
    }
}
