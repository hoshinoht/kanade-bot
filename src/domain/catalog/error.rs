use std::fmt;

/// Boss input that cannot be resolved to a canonical boss; the message is shown
/// to members as-is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BossParseError {
    message: String,
}

impl BossParseError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for BossParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BossParseError {}

/// A malformed boss catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BossTableError {
    message: String,
}

impl BossTableError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for BossTableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BossTableError {}
