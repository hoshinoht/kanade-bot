use std::io;

/// Operator-safe persona failure: never carries paths or persona text.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PersonaError {
    #[error("persona or profile ID is not a safe slug")]
    UnsafeId,
    #[error("persona alias is not a safe bare token")]
    UnsafeAlias,
    #[error("persona file is missing")]
    Missing,
    #[error("persona path must not be a symlink")]
    Symlink,
    #[error("persona path is not a regular file")]
    NotRegular,
    #[error("persona path escapes the persona directory")]
    Escape,
    #[error("persona file exceeds the size limit")]
    TooLarge,
    #[error("persona file must have exactly one hard link")]
    Linked,
    #[error("persona profiles directory has too many profile files")]
    TooManyProfiles,
    #[error("persona file is not valid UTF-8")]
    InvalidUtf8,
    #[error("persona file is unreadable ({0})")]
    Io(io::ErrorKind),
    #[error("{}", match .0 {
        YamlIssue::DuplicateKey => "persona YAML repeats a key",
        YamlIssue::UnknownField => "persona YAML has an unknown key",
        YamlIssue::MissingField => "persona YAML is missing a required key",
        YamlIssue::Other => "persona YAML is invalid",
    })]
    Yaml(YamlIssue),
    #[error("persona file is invalid: {0}")]
    Invalid(&'static str),
    #[error("persona is not in the catalog")]
    NotInCatalog,
}

/// YAML failure class; the parser message is dropped because it may quote values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YamlIssue {
    DuplicateKey,
    UnknownField,
    MissingField,
    Other,
}

impl YamlIssue {
    pub(crate) fn classify(message: &str) -> Self {
        let message = message.to_ascii_lowercase();
        if message.contains("duplicate") {
            Self::DuplicateKey
        } else if message.contains("unknown field") {
            Self::UnknownField
        } else if message.contains("missing field") {
            Self::MissingField
        } else {
            Self::Other
        }
    }
}
