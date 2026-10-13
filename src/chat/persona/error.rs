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
    Linked,
    TooManyProfiles,
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
        formatter.write_str(match self {
            Self::UnsafeId => "persona or profile ID is not a safe slug",
            Self::UnsafeAlias => "persona alias is not a safe bare token",
            Self::Missing => "persona file is missing",
            Self::Symlink => "persona path must not be a symlink",
            Self::NotRegular => "persona path is not a regular file",
            Self::Escape => "persona path escapes the persona directory",
            Self::TooLarge => "persona file exceeds the size limit",
            Self::Linked => "persona file must have exactly one hard link",
            Self::TooManyProfiles => "persona profiles directory has too many profile files",
            Self::InvalidUtf8 => "persona file is not valid UTF-8",
            Self::Io(kind) => return write!(formatter, "persona file is unreadable ({kind})"),
            Self::Yaml(YamlIssue::DuplicateKey) => "persona YAML repeats a key",
            Self::Yaml(YamlIssue::UnknownField) => "persona YAML has an unknown key",
            Self::Yaml(YamlIssue::MissingField) => "persona YAML is missing a required key",
            Self::Yaml(YamlIssue::Other) => "persona YAML is invalid",
            Self::Invalid(reason) => return write!(formatter, "persona file is invalid: {reason}"),
            Self::NotInCatalog => "persona is not in the catalog",
        })
    }
}

impl std::error::Error for PersonaError {}
