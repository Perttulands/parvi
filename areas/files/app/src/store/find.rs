//! Finding a file: by words in its path under every root as you type, or
//! by the text inside the files under one folder when asked.

use parvi_files_contract::Matches;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Find {
    pub query: String,
    /// Files with the words in their path; none while the node is looking.
    /// What is beside the asker arrives first and stands here until the
    /// find under every root answers.
    pub found: Option<Result<Matches, String>>,
    /// The find under every root has not answered yet.
    pub widening: bool,
    /// Enter came before any file was found: the first one found opens.
    pub open_first: bool,
    pub revision: u64,
    /// The text inside the files under one folder, once asked for.
    pub inside: Option<Inside>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inside {
    pub folder: String,
    /// None while the node is looking.
    pub found: Option<Result<Matches, String>>,
}

impl Find {
    pub fn new(query: &str, revision: u64) -> Self {
        Self {
            query: query.to_string(),
            found: None,
            widening: true,
            open_first: false,
            revision,
            inside: None,
        }
    }

    /// An answer for names: dropped when it answers a find no longer asked.
    pub fn answer(&mut self, revision: u64, found: Result<Matches, String>) {
        if self.revision == revision {
            self.found = Some(found);
            self.widening = false;
        }
    }

    /// The files beside the asker, which the node answers at once: shown
    /// until the find under every root answers, whose first files they
    /// are. Nothing beside, or a failure, leaves the wider find to say.
    pub fn answer_beside(&mut self, revision: u64, found: Result<Matches, String>) {
        if self.revision == revision
            && self.widening
            && let Ok(beside) = found
            && !beside.matches.is_empty()
        {
            self.found = Some(Ok(beside));
        }
    }

    /// The file a waiting Enter opens, once: the first one found.
    pub fn take_first(&mut self) -> Option<String> {
        if !self.open_first {
            return None;
        }
        let first = match &self.found {
            Some(Ok(m)) => m.matches.first().map(|m| m.path.clone()),
            _ => None,
        };
        // Nothing found by every root's find ends the wait too.
        if first.is_some() || !self.widening {
            self.open_first = false;
        }
        first
    }

    /// An answer for the text inside files, likewise.
    pub fn answer_inside(&mut self, revision: u64, found: Result<Matches, String>) {
        if self.revision == revision
            && let Some(inside) = self.inside.as_mut()
        {
            inside.found = Some(found);
        }
    }

    /// Every hit as the list numbers them: names, then lines inside files.
    pub fn matches(&self) -> Vec<parvi_files_contract::Match> {
        let of = |f: &Option<Result<Matches, String>>| match f {
            Some(Ok(m)) => m.matches.clone(),
            _ => vec![],
        };
        let mut all = of(&self.found);
        if let Some(inside) = &self.inside {
            all.extend(of(&inside.found));
        }
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_answers_are_dropped() {
        let mut f = Find::new("ab", 2);
        let none = || Matches {
            matches: vec![],
            truncated: false,
        };
        f.answer(1, Ok(none()));
        assert!(f.found.is_none());
        f.answer(2, Ok(none()));
        assert!(f.found.is_some());
    }

    fn hits(paths: &[&str]) -> Matches {
        Matches {
            matches: paths
                .iter()
                .map(|p| parvi_files_contract::Match {
                    path: p.to_string(),
                    name: p.rsplit('/').next().unwrap().to_string(),
                    line: None,
                    text: None,
                })
                .collect(),
            truncated: false,
        }
    }

    #[test]
    fn what_is_beside_shows_first_and_the_wider_find_takes_over() {
        let mut f = Find::new("no", 1);
        f.answer_beside(1, Ok(hits(&[])));
        assert!(f.found.is_none(), "nothing beside: still looking");
        f.answer_beside(1, Ok(hits(&["/here/note.txt"])));
        assert_eq!(f.matches().len(), 1);
        assert!(f.widening);
        f.answer(1, Ok(hits(&["/here/note.txt", "/far/note.txt"])));
        assert_eq!(f.matches().len(), 2);
        assert!(!f.widening);
        // A late answer from beside never takes the wider one's place.
        f.answer_beside(1, Ok(hits(&["/here/note.txt"])));
        assert_eq!(f.matches().len(), 2);
    }

    #[test]
    fn an_early_enter_opens_the_first_file_found() {
        let mut f = Find::new("no", 1);
        assert_eq!(f.take_first(), None, "nobody pressed Enter");
        f.open_first = true;
        assert_eq!(f.take_first(), None, "nothing found yet");
        assert!(f.open_first, "it waits");
        f.answer_beside(1, Ok(hits(&["/here/note.txt"])));
        assert_eq!(f.take_first().as_deref(), Some("/here/note.txt"));
        assert_eq!(f.take_first(), None, "once");
        // With nothing anywhere, the wait ends.
        let mut f = Find::new("zz", 2);
        f.open_first = true;
        f.answer(2, Ok(hits(&[])));
        assert_eq!(f.take_first(), None);
        assert!(!f.open_first);
    }
}
