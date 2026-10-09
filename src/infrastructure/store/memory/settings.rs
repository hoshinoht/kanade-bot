//! In-memory `SettingsStore`: the settings rows of a `config` table and
//! the append-only settings changes.

use std::collections::BTreeMap;

use crate::domain::scheduler::StoreError;
use crate::domain::settings::{SettingsChange, SettingsChangeQuery, SettingsStore, keys};

impl SettingsStore for super::MemoryScheduleStore {
    async fn settings_rows(&self) -> Result<BTreeMap<String, String>, StoreError> {
        Ok(self.config().clone())
    }

    async fn put_settings_rows(&self, rows: Vec<(String, String)>) -> Result<(), StoreError> {
        let result = async {
            refuse_unknown(&rows)?;
            self.config().extend(rows);
            Ok(())
        }
        .await;
        self.written
            .after(crate::infrastructure::store::Written::Settings, result)
    }

    async fn put_settings_rows_recorded(
        &self,
        rows: Vec<(String, String)>,
        change: SettingsChange,
    ) -> Result<u64, StoreError> {
        let result = async {
            refuse_unknown(&rows)?;
            refuse_change(&change)?;
            let mut config = self.config();
            let mut changes = self.changes();
            config.extend(rows);
            Ok(append(&mut changes, change))
        }
        .await;
        self.written
            .after(crate::infrastructure::store::Written::Settings, result)
    }

    async fn settings_changes(
        &self,
        query: SettingsChangeQuery,
    ) -> Result<Vec<SettingsChange>, StoreError> {
        let changes = self.changes();
        let mut found: Vec<SettingsChange> = changes
            .iter()
            .filter(|change| query.matches(change))
            .cloned()
            .collect();
        found.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
        Ok(found)
    }
}

/// As SQLite's `refuse_unknown`: nothing is written when a key is unknown.
pub(super) fn refuse_unknown(rows: &[(String, String)]) -> Result<(), StoreError> {
    match rows.iter().find(|(key, _)| !keys::is_setting(key)) {
        Some((key, _)) => Err(StoreError::Constraint(format!("{key:?} is not a setting"))),
        None => Ok(()),
    }
}

/// The checks SQLite's `append` makes before it writes.
pub(super) fn refuse_change(change: &SettingsChange) -> Result<(), StoreError> {
    if change.values.is_empty() {
        return Err(StoreError::Constraint(
            "a settings change names at least one row".into(),
        ));
    }
    // As SQLite's `to_iso`: an instant outside years 1..=9999 is refused.
    crate::domain::time::to_iso(&change.at)
        .map_err(|error| StoreError::Constraint(format!("{:?}: {error}", change.at)))?;
    Ok(())
}

/// Append a checked change with the next id; returns it.
pub(super) fn append(changes: &mut Vec<SettingsChange>, mut change: SettingsChange) -> u64 {
    change.id = changes.last().map_or(1, |last| last.id + 1);
    let id = change.id;
    changes.push(change);
    id
}

impl super::MemoryScheduleStore {
    pub(super) fn config(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn changes(&self) -> std::sync::MutexGuard<'_, Vec<SettingsChange>> {
        self.settings_changes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
