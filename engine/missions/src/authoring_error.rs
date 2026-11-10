//! Stable error codes for mission authoring.
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoringError {
    pub code: &'static str,
    pub detail: String,
}

impl AuthoringError {
    pub(crate) fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for AuthoringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}
impl std::error::Error for AuthoringError {}
impl From<crate::DefinitionError> for AuthoringError {
    fn from(error: crate::DefinitionError) -> Self {
        Self::new(
            match error {
                crate::DefinitionError::NotFound(_) => "NOT_FOUND",
                crate::DefinitionError::InvalidName(_) => "BAD_REQUEST",
                crate::DefinitionError::Ambiguous(_) => "AMBIGUOUS_SELECTOR",
                crate::DefinitionError::BrokenLink(_) => "BROKEN_LINK",
                crate::DefinitionError::InvalidSource(_) => "INVALID_DEFINITION_SOURCE",
                crate::DefinitionError::UnsupportedSchema(_) => "UNSUPPORTED_SCHEMA",
                crate::DefinitionError::Io(_) => "IO",
            },
            error.to_string(),
        )
    }
}
impl From<std::io::Error> for AuthoringError {
    fn from(error: std::io::Error) -> Self {
        Self::new("IO", error.to_string())
    }
}
