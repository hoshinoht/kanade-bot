//! Runtime settings over the v4-compatible `config` table (migration 0001).
//!
//! Resolution per key: the stored row, else the caller's env seed, else the
//! code default. The seed is a [`RuntimeSettings`] the caller builds from
//! [`RuntimeSettings::default`] with its environment applied, so an unset
//! variable keeps the code default. Writes go one [`Section`] at a time,
//! atomically.

mod codec;
pub mod keys;
mod model;

use std::collections::BTreeMap;
use std::fmt;

pub use codec::Section;
pub use model::{
    Chatbot, ContextRole, ContextSettings, DEFAULT_DEFLECTION_LINE, DEFAULT_RUN_MINUTES,
    LOCAL_CONTEXT_WARNING, LOCAL_CONTEXT_WARNING_TOKENS, MAX_CONTEXT_TOKENS, MAX_DEFLECTION_CHARS,
    MAX_PROFANITY_WORDS, MAX_ROLE_PROFILE_ASSIGNMENTS, Models, Notifications, OVERRIDE_RUN_MINUTES,
    PROFANITY_WORD_CHARS, Persona, Pings, Posting, Profanity, RUN_MINUTES, Rate, Reasoning,
    RoleModel, RoleProfileAssignment, RunLengthOverride, RunLengths, RuntimeSettings, Schedule,
    SelfService, SelfServiceMode, Watching, is_profanity_word,
};

use crate::domain::scheduler::StoreError;

/// The stored `(key, text)` rows a section writes, for diffing saved changes.
pub fn section_rows(section: &Section) -> Vec<(&'static str, String)> {
    codec::encode(section)
}

/// Raw access to the settings rows of the `config` table.
pub trait SettingsStore {
    /// Every stored row whose key is in [`keys::ALL`].
    fn settings_rows(
        &self,
    ) -> impl Future<Output = Result<BTreeMap<String, String>, StoreError>> + Send;

    /// Upsert `rows` in one transaction; a key outside [`keys::ALL`] is
    /// [`StoreError::Constraint`] and nothing is written.
    fn put_settings_rows(
        &self,
        rows: Vec<(String, String)>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsError {
    Store(StoreError),
    /// A stored (or about-to-be-stored) value does not decode.
    Malformed {
        key: &'static str,
        value: String,
        reason: String,
    },
    /// The section would not read back as written (e.g. unsorted countdowns
    /// or an alias with surrounding spaces); nothing was written.
    Unrepresentable {
        section: &'static str,
    },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(f),
            Self::Malformed { key, value, reason } => {
                write!(
                    f,
                    "setting `{key}` has an unreadable value {value:?}: {reason}"
                )
            }
            Self::Unrepresentable { section } => write!(
                f,
                "settings section `{section}` is not in stored form (it would read back differently)"
            ),
        }
    }
}

impl std::error::Error for SettingsError {}

impl From<StoreError> for SettingsError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// The effective settings: stored rows over `seed`.
///
/// # Errors
/// [`SettingsError::Malformed`] naming the first unreadable row, or the store's error.
pub async fn load_settings<S: SettingsStore>(
    store: &S,
    seed: &RuntimeSettings,
) -> Result<RuntimeSettings, SettingsError> {
    codec::resolve(&store.settings_rows().await?, seed)
}

/// Write every key of `section` in one transaction.
///
/// # Errors
/// [`SettingsError::Malformed`] / [`SettingsError::Unrepresentable`] before
/// anything is written, or the store's error.
pub async fn save_section<S: SettingsStore>(
    store: &S,
    section: &Section,
) -> Result<(), SettingsError> {
    let rows = codec::encode_checked(section)?
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    store.put_settings_rows(rows).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
