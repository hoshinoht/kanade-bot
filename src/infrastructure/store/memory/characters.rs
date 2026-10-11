//! In-memory `CharacterStore`, mirroring SQLite's `roster_characters`.

use std::collections::BTreeMap;

use super::settings::{append, refuse_change};
use crate::domain::characters::{
    Character, CharacterChange, CharacterRecord, CharacterStore, CharacterWrite, plan,
};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::SettingsChange;
use crate::infrastructure::store::Written;

/// Rows by id.
pub(super) type CharacterTable = BTreeMap<String, Character>;

fn of_member(table: &CharacterTable, member_id: &str) -> Vec<Character> {
    let mut rows: Vec<Character> = table
        .values()
        .filter(|row| row.member_id == member_id)
        .cloned()
        .collect();
    rows.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    rows
}

/// One character write: the outcome and its History record, checked but
/// not yet appended. Nothing changes on a refusal or an error.
pub(super) fn apply(
    table: &mut CharacterTable,
    change: CharacterChange,
    record: &CharacterRecord,
) -> Result<(CharacterWrite, Option<SettingsChange>), StoreError> {
    let current = table.get(change.id()).cloned();
    let member = change
        .row()
        .or(current.as_ref())
        .map(|row| row.member_id.clone());
    let mut others = member
        .map(|member| of_member(table, &member))
        .unwrap_or_default();
    others.retain(|other| other.id != change.id());
    let taken = change.row().is_some_and(|row| {
        table.values().any(|other| {
            other.id != row.id && other.world == row.world && other.ign_key() == row.ign_key()
        })
    });
    let planned = plan(change, current, &others, taken, record)?;
    let history = record.change(&planned.rows());
    if let Some(change) = &history {
        refuse_change(change)?;
    }
    for (before, after) in planned.rows() {
        match (before, after) {
            (_, Some(row)) => {
                table.insert(row.id.clone(), row.clone());
            }
            (Some(row), None) => {
                table.remove(&row.id);
            }
            (None, None) => {}
        }
    }
    Ok((planned.written, history))
}

impl super::MemoryScheduleStore {
    pub(super) fn characters(&self) -> std::sync::MutexGuard<'_, CharacterTable> {
        self.characters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl CharacterStore for super::MemoryScheduleStore {
    async fn member_characters(&self, member_id: &str) -> Result<Vec<Character>, StoreError> {
        Ok(of_member(&self.characters(), member_id))
    }

    async fn character(&self, id: &str) -> Result<Option<Character>, StoreError> {
        Ok(self.characters().get(id).cloned())
    }

    async fn apply_character(
        &self,
        change: CharacterChange,
        record: CharacterRecord,
    ) -> Result<CharacterWrite, StoreError> {
        let mut table = self.characters();
        let mut changes = self.changes();
        let (written, change) = apply(&mut table, change, &record)?;
        let recorded = change.is_some();
        if let Some(change) = change {
            append(&mut changes, change);
        }
        drop((table, changes));
        if recorded {
            self.written.notify(Written::Settings);
        }
        Ok(written)
    }
}
