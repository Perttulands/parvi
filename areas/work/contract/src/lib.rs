//! A work item's reference: which tracker holds it, where, and which item
//! (`docs/work.md`, References). The Work module resolves and reads them;
//! the mission engine records them on runs and briefs, so the syntax lives
//! in this crate that both use.
//!
//! - `bead:<node>:<folder>#<id>`: a Bead in the store of a project folder
//!   on a node, such as `bead:3f…:/srv/example-project#ctx-12`.
//! - `gh:<owner>/<repo>#<number>`: a GitHub issue. `<owner>/<repo>#<number>`
//!   is read the same way.
//! - A bare Bead id (`ctx-12`) is written by people and resolved by a node's
//!   Work configuration to the one store with that prefix.

use std::fmt;

/// A resolved reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkRef {
    /// A Bead in the store of `folder` on node `node`.
    Bead {
        node: String,
        folder: String,
        id: String,
    },
    /// A GitHub issue.
    Issue {
        owner: String,
        repo: String,
        number: u64,
    },
}

/// A reference as written: resolved, or a bare Bead id still to resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    Ref(WorkRef),
    Bead(String),
}

/// Why a text is not a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub String);

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Invalid {}

const FORMS: &str = "a work reference is bead:<node>:<folder>#<id>, gh:<owner>/<repo>#<number>, <owner>/<repo>#<number> or a Bead id such as ctx-12";

/// Reads a reference in any of its written forms.
pub fn parse(text: &str) -> Result<Written, Invalid> {
    let invalid = || Invalid(format!("{text:?}: {FORMS}"));
    if let Some(rest) = text.strip_prefix("bead:") {
        let (node, rest) = rest.split_once(':').ok_or_else(invalid)?;
        let (folder, id) = rest.rsplit_once('#').ok_or_else(invalid)?;
        if !is_node(node) || !is_absolute(folder) || !is_bead_id(id) {
            return Err(invalid());
        }
        return Ok(Written::Ref(WorkRef::Bead {
            node: node.into(),
            folder: folder.into(),
            id: id.into(),
        }));
    }
    if is_bead_id(text) {
        return Ok(Written::Bead(text.into()));
    }
    let issue = text.strip_prefix("gh:").unwrap_or(text);
    let (repo, number) = issue.rsplit_once('#').ok_or_else(invalid)?;
    let (owner, name) = repo.split_once('/').ok_or_else(invalid)?;
    let number: u64 = number
        .parse()
        .ok()
        .filter(|n| *n > 0 && number.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(invalid)?;
    if !is_repo_part(owner) || !is_repo_part(name) {
        return Err(invalid());
    }
    Ok(Written::Ref(WorkRef::Issue {
        owner: owner.into(),
        repo: name.into(),
        number,
    }))
}

/// Reads a resolved reference; a bare Bead id is refused, since only a
/// node's Work configuration can say which store holds it.
pub fn parse_resolved(text: &str) -> Result<WorkRef, Invalid> {
    match parse(text)? {
        Written::Ref(r) => Ok(r),
        Written::Bead(id) => Err(Invalid(format!(
            "{id} names no store: resolve it through the node's Work configuration"
        ))),
    }
}

impl WorkRef {
    /// `beads` or `github`.
    pub fn tracker(&self) -> &'static str {
        match self {
            WorkRef::Bead { .. } => "beads",
            WorkRef::Issue { .. } => "github",
        }
    }

    /// What `{bead}` and `{issue}` take in a brief: the Bead id, as Archon
    /// gives it, or `owner/repo#N`.
    pub fn short(&self) -> String {
        match self {
            WorkRef::Bead { id, .. } => id.clone(),
            WorkRef::Issue {
                owner,
                repo,
                number,
            } => format!("{owner}/{repo}#{number}"),
        }
    }
}

impl fmt::Display for WorkRef {
    /// The canonical form, which [`parse`] reads back.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkRef::Bead { node, folder, id } => write!(f, "bead:{node}:{folder}#{id}"),
            WorkRef::Issue {
                owner,
                repo,
                number,
            } => write!(f, "gh:{owner}/{repo}#{number}"),
        }
    }
}

impl Written {
    /// `bead` or `issue`: how a brief names it to an agent.
    pub fn noun(&self) -> &'static str {
        match self {
            Written::Ref(WorkRef::Issue { .. }) => "issue",
            _ => "bead",
        }
    }

    /// The line a brief gives an agent: the id, and the store's folder for
    /// a Bead whose store is known, so `bd` runs in the right one.
    pub fn describe(&self) -> String {
        match self {
            Written::Ref(WorkRef::Bead { folder, id, .. }) => format!("{id} (store: {folder})"),
            Written::Ref(r) => r.short(),
            Written::Bead(id) => id.clone(),
        }
    }
}

/// Archon's safe Beads issue id: a lowercase prefix, a hyphen and an id,
/// with dotted child numbers (`archon-3yd.4`).
pub fn is_bead_id(id: &str) -> bool {
    let Some((prefix, rest)) = id.split_once('-') else {
        return false;
    };
    let mut parts = rest.split('.');
    let first = parts.next().unwrap_or("");
    let lower_digits = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    };
    prefix
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase())
        && lower_digits(prefix)
        && !first.is_empty()
        && lower_digits(first)
        && parts.all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// GitHub's rule, loosely: letters, digits, `-`, `_` and `.`, not starting
/// with `-`. Enough to keep anything else out of a `gh` command line.
pub fn is_repo_part(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// A node id: letters, digits, `-`, `_` and `.`.
fn is_node(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// An absolute folder on Linux or Windows, with no control characters.
pub fn is_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    let rooted = path.starts_with('/')
        || path.starts_with("\\\\")
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'\\' | b'/'));
    rooted && !path.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bead(node: &str, folder: &str, id: &str) -> WorkRef {
        WorkRef::Bead {
            node: node.into(),
            folder: folder.into(),
            id: id.into(),
        }
    }

    #[test]
    fn reads_back_what_it_writes() {
        for r in [
            bead("3f2a", "/srv/example-project", "ctx-12"),
            bead("n1", "/srv/my #1 project", "srv-ubw.8.64"),
            bead("n1", "C:\\work\\x", "a1-b2"),
            WorkRef::Issue {
                owner: "example-owner".into(),
                repo: "parvi".into(),
                number: 179,
            },
        ] {
            assert_eq!(parse(&r.to_string()), Ok(Written::Ref(r.clone())));
        }
    }

    #[test]
    fn reads_written_forms() {
        assert_eq!(parse("ctx-12"), Ok(Written::Bead("ctx-12".into())));
        assert_eq!(
            parse("example-owner/parvi#7").unwrap(),
            parse("gh:example-owner/parvi#7").unwrap()
        );
        let r = parse_resolved("gh:a/b#7").unwrap();
        assert_eq!((r.tracker(), r.short()), ("github", "a/b#7".into()));
        let r = parse_resolved("bead:n:/srv/x#x-1.2").unwrap();
        assert_eq!((r.tracker(), r.short()), ("beads", "x-1.2".into()));
        assert!(parse_resolved("ctx-12").is_err());
    }

    #[test]
    fn describes_to_an_agent() {
        let written = parse("bead:n:/srv/x#x-1").unwrap();
        assert_eq!(
            (written.noun(), written.describe()),
            ("bead", "x-1 (store: /srv/x)".into())
        );
        let written = parse("a/b#3").unwrap();
        assert_eq!(
            (written.noun(), written.describe()),
            ("issue", "a/b#3".into())
        );
        assert_eq!(parse("x-1").unwrap().describe(), "x-1");
    }

    #[test]
    fn refuses_anything_else() {
        for bad in [
            "",
            "ctx",
            "CTX-1",
            "ctx-1.",
            "ctx-a.b",
            "bead:/srv/x#ctx-1",
            "bead:n:relative#ctx-1",
            "bead:n:/srv/x#CTX",
            "bead:n:/srv/x\n#ctx-1",
            "gh:a/b",
            "a/b#0",
            "a/b#-1",
            "a/b#+1",
            "a/b/c#1",
            "-F/x#1",
            "a/$(rm)#1",
            "issue:a/b#1",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}

pub mod item;
pub mod model;
pub use item::{Family, Item};
pub use model::*;
pub const WATCH: u8 = 0x98;
pub const SNAPSHOT: u8 = 0x90;
pub const KINDS: parvi_core::frame::Kinds = &[WATCH..=WATCH];
pub const BAD_REQUEST: &str = "bad_request";
pub const TRACKER_ERROR: &str = "tracker_error";
