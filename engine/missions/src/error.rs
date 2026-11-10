//! Why a definition file could not be read.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum DefinitionError {
    /// No such file, or no mission with that slug or id.
    NotFound(String),
    /// A slug or id that cannot name a file, such as one holding a slash.
    InvalidName(String),
    /// More than one mission answers to a selector.
    Ambiguous(String),
    /// A symlink whose target cannot be opened: "<link> is a symlink to
    /// <target>, which does not exist".
    BrokenLink(String),
    /// Not strict TOML (or JSON) of its schema.
    InvalidSource(String),
    UnsupportedSchema(String),
    Io(String),
}

impl DefinitionError {
    /// The stable code clients branch on.
    pub fn code(&self) -> &'static str {
        match self {
            DefinitionError::NotFound(_) => "not_found",
            DefinitionError::InvalidName(_) => "invalid_name",
            DefinitionError::Ambiguous(_) => "ambiguous_selector",
            DefinitionError::BrokenLink(_) => "broken_link",
            DefinitionError::InvalidSource(_) => "invalid_definition_source",
            DefinitionError::UnsupportedSchema(_) => "unsupported_schema",
            DefinitionError::Io(_) => "io",
        }
    }

    fn detail(&self) -> &str {
        match self {
            DefinitionError::NotFound(detail)
            | DefinitionError::InvalidName(detail)
            | DefinitionError::Ambiguous(detail)
            | DefinitionError::BrokenLink(detail)
            | DefinitionError::InvalidSource(detail)
            | DefinitionError::UnsupportedSchema(detail)
            | DefinitionError::Io(detail) => detail,
        }
    }
}

impl fmt::Display for DefinitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // A broken link's words stand alone, as the listing shows them.
            DefinitionError::BrokenLink(detail) => f.write_str(detail),
            other => write!(f, "{}: {}", other.code(), other.detail()),
        }
    }
}

impl std::error::Error for DefinitionError {}
