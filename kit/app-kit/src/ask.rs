//! The few requests that cross pages, as plain data, so no page depends on
//! another: a page asks through its Host, and the page that answers that
//! ask gets it. A page declares what it answers in [`crate::Page::ANSWERS`].

use parvi_core::NodeId;

/// Something a page wants done where another page does it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Hand a reference to a session, as Send to Session does: the one
    /// `target` names, or else the one the answering page has in focus.
    Send {
        reference: String,
        target: Option<SessionTarget>,
    },
    /// Open a path on a node in the file viewer: as its own page, or
    /// `beside` the page that asked.
    OpenPath {
        node: NodeId,
        path: String,
        beside: bool,
    },
    /// Show a thing where it lives: `what` names its kind (a path, an item,
    /// a run, a document), `id` which one, on `node` if
    /// one is named, at `part` of it if one is (a run's step).
    Reveal {
        what: String,
        id: String,
        node: Option<NodeId>,
        part: Option<String>,
    },
}

/// A terminal session, wherever it runs: its node, whose tmux server (the
/// Unix user, empty for the node's own) and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTarget {
    pub node: NodeId,
    pub user: String,
    pub name: String,
}

/// An ask a page answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answers {
    Send,
    OpenPath,
    /// Reveals of this kind of thing.
    Reveal(&'static str),
}

impl Ask {
    /// Whether a page that answers `a` answers this.
    pub fn answered_by(&self, a: &Answers) -> bool {
        match (self, a) {
            (Ask::Send { .. }, Answers::Send) | (Ask::OpenPath { .. }, Answers::OpenPath) => true,
            (Ask::Reveal { what, .. }, Answers::Reveal(w)) => what == w,
            _ => false,
        }
    }

    /// What it asks, in words for the status line.
    pub fn describe(&self) -> String {
        match self {
            Ask::Send { .. } => "send to a session".into(),
            Ask::OpenPath { path, .. } => format!("open {path}"),
            Ask::Reveal { what, id, .. } => format!("show the {what} {id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ask_goes_to_what_answers_its_kind() {
        let send = Ask::Send {
            reference: "x".into(),
            target: None,
        };
        let run = Ask::Reveal {
            what: "run".into(),
            id: "r1".into(),
            node: None,
            part: None,
        };
        assert!(send.answered_by(&Answers::Send));
        assert!(!send.answered_by(&Answers::OpenPath));
        assert!(run.answered_by(&Answers::Reveal("run")));
        assert!(!run.answered_by(&Answers::Reveal("item")));
        assert_eq!(run.describe(), "show the run r1");
    }
}
