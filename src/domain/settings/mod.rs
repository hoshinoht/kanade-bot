//! Runtime settings over the v4-compatible `config` table (migration 0001).
//!
//! Resolution per key: the stored row, else the caller's env seed, else the
//! code default. The seed is a [`RuntimeSettings`] the caller builds from
//! [`RuntimeSettings::default`] with its environment applied, so an unset
//! variable keeps the code default. Writes go one [`Section`] at a time,
//! atomically.

mod audit;
mod codec;
pub mod keys;
mod model;

use std::collections::BTreeMap;
use std::fmt;

pub use audit::{RowDiff, SettingsChange, SettingsChangeQuery, diff_rows};
pub use codec::{IdList, Section};
pub use model::{
    Chatbot, ContextRole, ContextSettings, DEFAULT_DEFLECTION_LINE, DEFAULT_RUN_MINUTES,
    LOCAL_CONTEXT_WARNING, LOCAL_CONTEXT_WARNING_TOKENS, MAX_CONTEXT_TOKENS, MAX_DEFLECTION_CHARS,
    MAX_PROFANITY_WORDS, MAX_ROLE_PROFILE_ASSIGNMENTS, MessageStyle, Models, Notifications,
    OVERRIDE_RUN_MINUTES, PROFANITY_WORD_CHARS, Persona, Pings, Posting, Profanity, RUN_MINUTES,
    Rate, Reasoning, RoleModel, RoleProfileAssignment, RunLengthOverride, RunLengths,
    RuntimeSettings, Schedule, SelfService, SelfServiceMode, Watching, is_profanity_word,
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

    /// As [`Self::put_settings_rows`], appending `change` in the same
    /// transaction; returns the change's id.
    fn put_settings_rows_recorded(
        &self,
        rows: Vec<(String, String)>,
        change: SettingsChange,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// Recorded section saves matching `query`, newest first.
    fn settings_changes(
        &self,
        query: SettingsChangeQuery,
    ) -> impl Future<Output = Result<Vec<SettingsChange>, StoreError>> + Send;
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

/// [`save_section`], appending `change` (if any) in the same transaction, so
/// a refused save records nothing.
///
/// # Errors
/// As [`save_section`].
pub async fn save_section_recorded<S: SettingsStore>(
    store: &S,
    section: &Section,
    change: Option<SettingsChange>,
) -> Result<(), SettingsError> {
    let Some(change) = change else {
        return save_section(store, section).await;
    };
    let rows = codec::encode_checked(section)?
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    store.put_settings_rows_recorded(rows, change).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
