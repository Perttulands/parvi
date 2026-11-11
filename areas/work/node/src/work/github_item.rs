//! Reads GitHub's answer to `item.graphql` into an [`Item`] with its family.

use anyhow::{Context, Result, bail};
use parvi_work_ref::WorkRef;
use serde::Deserialize;

use super::item::{Family, Item};

/// The query for one issue with its family.
pub const QUERY: &str = include_str!("item.graphql");

/// Parses `gh api graphql`'s answer for issue `number` of `owner/repo`.
pub fn parse(json: &str, owner: &str, repo: &str) -> Result<Item> {
    let answer: Answer = serde_json::from_str(json).context("GitHub's answer is not JSON")?;
    if !answer.errors.is_empty() {
        let missing = answer.errors.iter().all(|e| {
            e.message
                .starts_with("Could not resolve to an Issue with the number of ")
                || (e.kind == "NOT_FOUND" && e.path.last().is_some_and(|p| p == "issue"))
        });
        let messages: Vec<_> = answer.errors.into_iter().map(|e| e.message).collect();
        let message = messages.join("; ");
        if missing {
            return Err(super::MissingItem(message).into());
        }
        bail!("{message}");
    }
    let repository = answer
        .data
        .and_then(|d| d.repository)
        .context("GitHub sent no repository")?;
    let raw = repository
        .issue
        .ok_or_else(|| super::MissingItem(format!("no issue in {owner}/{repo}")))?;
    let here = format!("{owner}/{repo}");
    let reference = |linked: &Linked| -> String {
        let (owner, repo) = linked
            .repository
            .as_ref()
            .and_then(|r| r.name_with_owner.split_once('/'))
            .unwrap_or_else(|| here.split_once('/').expect("owner/repo"));
        issue(owner, repo, linked.number)
    };
    Ok(Item {
        reference: issue(owner, repo, raw.number),
        id: raw.number.to_string(),
        title: raw.title,
        status: raw.state.to_lowercase(),
        kind: None,
        priority: None,
        url: Some(raw.url),
        author: raw.author.map(|a| a.login),
        assignees: raw.assignees.nodes.into_iter().map(|u| u.login).collect(),
        labels: raw.labels.nodes.into_iter().map(|l| l.name).collect(),
        created: raw.created_at,
        updated: raw.updated_at,
        closed: raw.closed_at,
        body: raw.body,
        family: Family {
            parent: raw.parent.as_ref().map(reference),
            children: raw.sub_issues.nodes.iter().map(reference).collect(),
            blockers: raw.blocked_by.nodes.iter().map(reference).collect(),
            blocking: raw.blocking.nodes.iter().map(reference).collect(),
            pulls: raw
                .closed_by_pull_requests_references
                .nodes
                .iter()
                .map(|p| issue(owner, repo, p.number))
                .collect(),
        },
    })
}

fn issue(owner: &str, repo: &str, number: u64) -> String {
    WorkRef::Issue {
        owner: owner.into(),
        repo: repo.into(),
        number,
    }
    .to_string()
}

#[derive(Deserialize)]
struct Answer {
    data: Option<Data>,
    #[serde(default)]
    errors: Vec<Message>,
}

#[derive(Deserialize)]
struct Message {
    message: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    path: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Data {
    repository: Option<Repository>,
}

#[derive(Deserialize)]
struct Repository {
    issue: Option<RawIssue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawIssue {
    number: u64,
    title: String,
    url: String,
    state: String,
    body: String,
    created_at: String,
    updated_at: String,
    closed_at: Option<String>,
    author: Option<Login>,
    labels: Nodes<Named>,
    assignees: Nodes<Login>,
    parent: Option<Linked>,
    sub_issues: Nodes<Linked>,
    blocked_by: Nodes<Linked>,
    blocking: Nodes<Linked>,
    closed_by_pull_requests_references: Nodes<Numbered>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
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
    number: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Linked {
    number: u64,
    repository: Option<NameWithOwner>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameWithOwner {
    name_with_owner: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/work/item.json");

    #[test]
    fn reads_an_issue_with_the_pull_requests_that_close_it() {
        let item = parse(FIXTURE, "example-owner", "parvi").unwrap();
        assert_eq!(item.reference, "gh:example-owner/parvi#51");
        assert_eq!((item.id.as_str(), item.status.as_str()), ("51", "closed"));
        assert_eq!(item.family.pulls, vec!["gh:example-owner/parvi#58"]);
        assert!(item.family.parent.is_none() && item.family.children.is_empty());
    }

    #[test]
    fn names_family_in_other_repositories_by_their_own() {
        let mut answer: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        let issue = &mut answer["data"]["repository"]["issue"];
        issue["parent"] = serde_json::json!({"number": 1, "repository": {"nameWithOwner": "example-owner/parvi"}});
        issue["subIssues"]["nodes"] = serde_json::json!([{"number": 2, "repository": {"nameWithOwner": "example-owner/parvi"}}]);
        issue["blockedBy"]["nodes"] =
            serde_json::json!([{"number": 3, "repository": {"nameWithOwner": "cli/cli"}}]);
        issue["blocking"]["nodes"] = serde_json::json!([{"number": 4, "repository": null}]);
        let item = parse(&answer.to_string(), "example-owner", "parvi").unwrap();
        assert_eq!(
            item.family.parent.as_deref(),
            Some("gh:example-owner/parvi#1")
        );
        assert_eq!(item.family.children, vec!["gh:example-owner/parvi#2"]);
        assert_eq!(item.family.blockers, vec!["gh:cli/cli#3"]);
        assert_eq!(item.family.blocking, vec!["gh:example-owner/parvi#4"]);
    }

    #[test]
    fn a_missing_issue_is_githubs_message() {
        let answer = r#"{"data":{"repository":{"issue":null}},"errors":[{"message":"Could not resolve to an Issue with the number of 99999."}]}"#;
        let error = parse(answer, "a", "b").unwrap_err().to_string();
        assert!(error.contains("99999"), "{error}");
        let error = parse(answer, "a", "b").unwrap_err();
        assert!(error.downcast_ref::<super::super::MissingItem>().is_some());
        let failure =
            r#"{"data":{"repository":null},"errors":[{"message":"rate limit exceeded"}]}"#;
        let error = parse(failure, "a", "b").unwrap_err();
        assert!(error.downcast_ref::<super::super::MissingItem>().is_none());
    }
}
