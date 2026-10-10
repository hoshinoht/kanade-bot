//! UUID row identifiers and the short forms people type.
//!
//! Rows are keyed by UUIDv4 so ids survive exports and merges and cannot be
//! guessed. Chat shows the first eight hex characters (`#a1b2c3d4`), and every id
//! argument accepts any unique prefix of at least [`MIN_PREFIX`] characters.

use std::fmt;

use uuid::Uuid;

use super::pytext::strip;

/// Shortest prefix accepted from a user: 65k values, short enough for a phone.
pub const MIN_PREFIX: usize = 4;
/// Length of the displayed short id.
pub const SHORT_LENGTH: usize = 8;

/// Source of new row ids; the seam that keeps randomness out of pure callers.
pub trait IdGenerator {
    /// A fresh id as a lowercase dashed UUID string.
    fn new_id(&mut self) -> String;
}

/// Random UUIDv4 ids from the operating system RNG.
#[derive(Clone, Copy, Debug, Default)]
pub struct RandomIds;

impl IdGenerator for RandomIds {
    fn new_id(&mut self) -> String {
        Uuid::new_v4().hyphenated().to_string()
    }
}

/// Id-resolution failures, with v4's messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdError {
    /// Fewer than [`MIN_PREFIX`] hex characters were given.
    TooShort { text: String },
    /// No candidate starts with the prefix.
    NotFound { text: String },
    /// More than one candidate starts with the prefix.
    Ambiguous {
        text: String,
        candidates: Vec<String>,
    },
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { text } => {
                write!(
                    f,
                    "`{text}` is too short - give at least {MIN_PREFIX} characters"
                )
            }
            Self::NotFound { text } => write!(f, "nothing matches `{text}`"),
            Self::Ambiguous { text, candidates } => {
                write!(f, "`{text}` matches {} rows", candidates.len())
            }
        }
    }
}

impl std::error::Error for IdError {}

/// Lowercase hex used for comparison, with whitespace, leading `#` and dashes removed.
pub fn canonical(value: &str) -> String {
    strip(value)
        .trim_start_matches('#')
        .replace('-', "")
        .to_lowercase()
}

/// The display form: the first eight characters of [`canonical`].
pub fn short_id(value: &str) -> String {
    canonical(value).chars().take(SHORT_LENGTH).collect()
}

/// The display form with its `#`, e.g. `#a1b2c3d4`.
pub fn tag(value: &str) -> String {
    format!("#{}", short_id(value))
}

/// Resolve a full id or unique prefix to exactly one candidate.
///
/// Case-insensitive; a leading `#` and dashes are ignored so anything the bot
/// prints can be pasted back. An exact match wins over longer prefix matches.
///
/// # Errors
/// [`IdError::TooShort`], [`IdError::NotFound`] or [`IdError::Ambiguous`].
pub fn resolve_id<T: AsRef<str>>(
    text: &str,
    candidates: impl IntoIterator<Item = T>,
) -> Result<T, IdError> {
    let prefix = canonical(text);
    if prefix.chars().count() < MIN_PREFIX {
        return Err(IdError::TooShort {
            text: text.to_owned(),
        });
    }
    let mut matches = Vec::new();
    for candidate in candidates {
        let key = canonical(candidate.as_ref());
        if key == prefix {
            return Ok(candidate);
        }
        if key.starts_with(&prefix) {
            matches.push(candidate);
        }
    }
    match matches.len() {
        0 => Err(IdError::NotFound {
            text: text.to_owned(),
        }),
        1 => Ok(matches.remove(0)),
        _ => Err(IdError::Ambiguous {
            text: text.to_owned(),
            candidates: matches.iter().map(|c| c.as_ref().to_owned()).collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_ids_are_canonical_uuid4() {
        let id = RandomIds.new_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id, id.to_lowercase());
        assert_eq!(Uuid::parse_str(&id).unwrap().get_version_num(), 4);
    }

    #[test]
    fn exact_match_beats_a_later_prefix_match() {
        let ids = ["abcd1234", "abcd"];
        assert_eq!(resolve_id("ABCD", ids), Ok("abcd"));
        let error = resolve_id("abc-d", ["abcd1", "abcd2"]).unwrap_err();
        assert_eq!(
            error,
            IdError::Ambiguous {
                text: "abc-d".into(),
                candidates: vec!["abcd1".into(), "abcd2".into()]
            }
        );
    }
}
