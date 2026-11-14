//! Finding a file: by words in its path under every root as you type, or
//! by the text inside the files under one folder when asked.

use parvi_files_contract::Matches;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Find {
    pub query: String,
    /// Files with the words in their path; none while the node is looking.
    pub found: Option<Result<Matches, String>>,
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
            revision,
            inside: None,
        }
    }

    /// An answer for names: dropped when it answers a find no longer asked.
    pub fn answer(&mut self, revision: u64, found: Result<Matches, String>) {
        if self.revision == revision {
            self.found = Some(found);
        }
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
}
