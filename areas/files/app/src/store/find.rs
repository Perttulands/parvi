//! Finding a file: by words in its path under every root as you type, or
//! by the text inside the files under one folder when asked.

use parvi_files_contract::Matches;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Find {
    pub query: String,
    /// The folder whose files' text is searched; none for names.
    pub text: Option<String>,
    /// None while the node is looking.
    pub found: Option<Result<Matches, String>>,
    pub revision: u64,
}

impl Find {
    pub fn new(query: &str, text: Option<String>, revision: u64) -> Self {
        Self {
            query: query.to_string(),
            text,
            found: None,
            revision,
        }
    }

    /// An answer: dropped when it answers a find no longer asked.
    pub fn answer(&mut self, revision: u64, found: Result<Matches, String>) {
        if self.revision == revision {
            self.found = Some(found);
        }
    }

    pub fn matches(&self) -> &[parvi_files_contract::Match] {
        match &self.found {
            Some(Ok(m)) => &m.matches,
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_answers_are_dropped() {
        let mut f = Find::new("ab", None, 2);
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
