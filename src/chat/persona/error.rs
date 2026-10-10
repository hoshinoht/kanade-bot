use std::{fmt, io};

/// Operator-safe persona failure: never carries paths or persona text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersonaError {
    UnsafeId,
    UnsafeAlias,
    Missing,
    Symlink,
    NotRegular,
    Escape,
    TooLarge,
    InvalidUtf8,
    Io(io::ErrorKind),
    Yaml(YamlIssue),
    Invalid(&'static str),
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

impl fmt::Display for PersonaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsafeId => formatter.write_str("persona or profile ID is not a safe slug"),
            Self::UnsafeAlias => formatter.write_str("persona alias is not a safe bare token"),
            Self::Missing => formatter.write_str("persona file is missing"),
            Self::Symlink => formatter.write_str("persona path must not be a symlink"),
            Self::NotRegular => formatter.write_str("persona path is not a regular file"),
            Self::Escape => formatter.write_str("persona path escapes the persona directory"),
            Self::TooLarge => formatter.write_str("persona file exceeds the size limit"),
            Self::InvalidUtf8 => formatter.write_str("persona file is not valid UTF-8"),
            Self::Io(kind) => write!(formatter, "persona file is unreadable ({kind})"),
            Self::Yaml(YamlIssue::DuplicateKey) => {
                formatter.write_str("persona YAML repeats a key")
            }
            Self::Yaml(YamlIssue::UnknownField) => {
                formatter.write_str("persona YAML has an unknown key")
            }
            Self::Yaml(YamlIssue::MissingField) => {
                formatter.write_str("persona YAML is missing a required key")
            }
            Self::Yaml(YamlIssue::Other) => formatter.write_str("persona YAML is invalid"),
            Self::Invalid(reason) => write!(formatter, "persona file is invalid: {reason}"),
            Self::NotInCatalog => formatter.write_str("persona is not in the catalog"),
        }
    }
}

impl std::error::Error for PersonaError {}
