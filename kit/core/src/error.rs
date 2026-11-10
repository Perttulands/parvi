//! The one error shape (ARCHITECTURE.md, Errors): a stable snake_case code
//! a program can match on, and a message for a person. Each area owns its
//! codes in its contract.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Error {
    pub code: String,
    pub message: String,
}

impl Error {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_as_its_message_and_code_and_travels_as_json() {
        let error = Error::new("no_session", "no session alpha");
        assert_eq!(error.to_string(), "no session alpha (no_session)");
        let json = r#"{"code":"no_session","message":"no session alpha"}"#;
        assert_eq!(serde_json::to_string(&error).unwrap(), json);
        assert_eq!(serde_json::from_str::<Error>(json).unwrap(), error);
    }
}
