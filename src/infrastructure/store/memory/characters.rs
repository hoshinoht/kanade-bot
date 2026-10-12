//! In-memory `CharacterStore`, mirroring SQLite's `roster_characters` and
//! `roster_run_picks` (a deleted character's picks go with it, as SQLite's
//! cascade does).

use std::collections::BTreeMap;

use super::settings::{append, refuse_change};
use crate::domain::characters::{
    Character, CharacterChange, CharacterRecord, CharacterStore, CharacterWrite, PickChange,
    PickWrite, RunPick, plan, plan_pick,
};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::SettingsChange;
use crate::infrastructure::store::Written;

/// Characters by id and their run picks by `(run, member)`, under one lock.
#[derive(Debug, Default)]
pub(super) struct CharacterTable {
    rows: BTreeMap<String, Character>,
    picks: BTreeMap<(String, String), RunPick>,
}

fn of_member(table: &CharacterTable, member_id: &str) -> Vec<Character> {
    let mut rows: Vec<Character> = table
        .rows
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
    let current = table.rows.get(change.id()).cloned();
    let member = change
        .row()
        .or(current.as_ref())
        .map(|row| row.member_id.clone());
    let mut others = member
        .map(|member| of_member(table, &member))
        .unwrap_or_default();
    others.retain(|other| other.id != change.id());
    let taken = change.row().is_some_and(|row| {
        table.rows.values().any(|other| {
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
                table.rows.insert(row.id.clone(), row.clone());
            }
            (Some(row), None) => {
                table.rows.remove(&row.id);
                table.picks.retain(|_, pick| pick.character_id != row.id);
            }
            (None, None) => {}
        }
    }
    Ok((planned.written, history))
}

/// One pick write: the outcome and its History record, checked but not
/// yet appended. Nothing changes on a refusal or an error.
pub(super) fn apply_pick(
    table: &mut CharacterTable,
    change: PickChange,
    record: &CharacterRecord,
) -> Result<(PickWrite, Option<SettingsChange>), StoreError> {
    let (run_id, member_id) = change.slot();
    let slot = (run_id.to_owned(), member_id.to_owned());
    let current = table.picks.get(&slot).cloned();
    let current_character = current
        .as_ref()
        .and_then(|pick| table.rows.get(&pick.character_id));
    let character = change.character_id().and_then(|id| table.rows.get(id));
    let planned = plan_pick(change, current, current_character, character, record)?;
    if let Some(change) = &planned.change {
        refuse_change(change)?;
    }
    match (planned.before, planned.after) {
        (_, Some(after)) => {
            table.picks.insert(slot, after);
        }
        (Some(_), None) => {
            table.picks.remove(&slot);
        }
        (None, None) => {}
    }
    Ok((planned.written, planned.change))
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
        Ok(self.characters().rows.get(id).cloned())
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

    async fn run_picks(&self, run_ids: &[String]) -> Result<Vec<RunPick>, StoreError> {
        Ok(self
            .characters()
            .picks
            .values()
            .filter(|pick| run_ids.contains(&pick.run_id))
            .cloned()
            .collect())
    }

    async fn apply_pick(
        &self,
        change: PickChange,
        record: CharacterRecord,
    ) -> Result<PickWrite, StoreError> {
        let mut table = self.characters();
        let mut changes = self.changes();
        let (written, change) = apply_pick(&mut table, change, &record)?;
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
