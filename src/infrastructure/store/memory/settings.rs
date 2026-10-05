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
            if let Some((key, _)) = rows.iter().find(|(key, _)| !keys::is_setting(key)) {
                return Err(StoreError::Constraint(format!("{key:?} is not a setting")));
            }
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
        mut change: SettingsChange,
    ) -> Result<u64, StoreError> {
        let result = async {
            if let Some((key, _)) = rows.iter().find(|(key, _)| !keys::is_setting(key)) {
                return Err(StoreError::Constraint(format!("{key:?} is not a setting")));
            }
            if change.values.is_empty() {
                return Err(StoreError::Constraint(
                    "a settings change names at least one row".into(),
                ));
            }
            // As SQLite's `to_iso`: an instant outside years 1..=9999 is refused.
            crate::domain::time::to_iso(&change.at)
                .map_err(|error| StoreError::Constraint(format!("{:?}: {error}", change.at)))?;
            let mut config = self.config();
            let mut changes = self
                .settings_changes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            change.id = changes.last().map_or(1, |last| last.id + 1);
            config.extend(rows);
            let id = change.id;
            changes.push(change);
            Ok(id)
        }
        .await;
        self.written
            .after(crate::infrastructure::store::Written::Settings, result)
    }

    async fn settings_changes(
        &self,
        query: SettingsChangeQuery,
    ) -> Result<Vec<SettingsChange>, StoreError> {
        let changes = self
            .settings_changes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut found: Vec<SettingsChange> = changes
            .iter()
            .filter(|change| query.matches(change))
            .cloned()
            .collect();
        found.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
        Ok(found)
    }
}

impl super::MemoryScheduleStore {
    fn config(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
