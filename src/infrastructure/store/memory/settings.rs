//! In-memory `SettingsStore`: the settings rows of a `config` table.

use std::collections::BTreeMap;

use crate::domain::scheduler::StoreError;
use crate::domain::settings::{SettingsStore, keys};

impl SettingsStore for super::MemoryScheduleStore {
    async fn settings_rows(&self) -> Result<BTreeMap<String, String>, StoreError> {
        Ok(self.config().clone())
    }

    async fn put_settings_rows(&self, rows: Vec<(String, String)>) -> Result<(), StoreError> {
        if let Some((key, _)) = rows.iter().find(|(key, _)| !keys::is_setting(key)) {
            return Err(StoreError::Constraint(format!("{key:?} is not a setting")));
        }
        self.config().extend(rows);
        Ok(())
    }
}

impl super::MemoryScheduleStore {
    fn config(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
