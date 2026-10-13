//! In-memory `ReplayStore`, mirroring SQLite's `idempotency_replays`: a
//! write first drops the rows expired at its `created_at`, and a live row of
//! another request refuses.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::members::portal;
use super::settings::{append, refuse_change};
use crate::domain::characters::{
    CharacterChange, CharacterRecord, CharacterWrite, PickChange, PickWrite,
};
use crate::domain::members::PortalEdit;
use crate::domain::scheduler::StoreError;
use crate::domain::settings::SettingsChange;
use crate::infrastructure::store::Written;
use crate::infrastructure::store::replays::{
    KeyedWrite, PortalRecord, PortalWrite, ReplayScope, ReplayStore, StoredReplay, keyed, replayed,
};
use crate::infrastructure::store::rules::refuse_unknown_settings;

pub(super) type ReplayTable = BTreeMap<(ReplayScope, String, String), StoredReplay>;

fn prune(table: &mut ReplayTable, now: DateTime<Utc>) -> u64 {
    let before = table.len();
    table.retain(|_, replay| replay.live(now));
    (before - table.len()) as u64
}

/// The checks `put` makes before it writes, after the expired rows are gone.
fn refuse_other_request(table: &ReplayTable, replay: &StoredReplay) -> Result<(), StoreError> {
    replay.check()?;
    match table.get(&(replay.scope, replay.actor.clone(), replay.key.clone())) {
        Some(held) if held.live(replay.created_at) && held.request != replay.request => Err(
            StoreError::Constraint("replay key holds another live request".into()),
        ),
        _ => Ok(()),
    }
}

fn put(table: &mut ReplayTable, replay: StoredReplay) {
    prune(table, replay.created_at);
    table.insert(
        (replay.scope, replay.actor.clone(), replay.key.clone()),
        replay,
    );
}

impl super::MemoryScheduleStore {
    fn replays(&self) -> std::sync::MutexGuard<'_, ReplayTable> {
        self.replays
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ReplayStore for super::MemoryScheduleStore {
    async fn replay(
        &self,
        scope: ReplayScope,
        actor: &str,
        key: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<StoredReplay>, StoreError> {
        Ok(self
            .replays()
            .get(&(scope, actor.to_owned(), key.to_owned()))
            .filter(|replay| replay.live(now))
            .cloned())
    }

    async fn put_replay(&self, replay: StoredReplay) -> Result<(), StoreError> {
        let mut table = self.replays();
        refuse_other_request(&table, &replay)?;
        put(&mut table, replay);
        Ok(())
    }

    async fn put_settings_rows_replayed(
        &self,
        rows: Vec<(String, String)>,
        change: Option<SettingsChange>,
        replay: StoredReplay,
    ) -> Result<(), StoreError> {
        let result = async {
            refuse_unknown_settings(&rows)?;
            if let Some(change) = &change {
                refuse_change(change)?;
            }
            let mut config = self.config();
            let mut changes = self.changes();
            let mut table = self.replays();
            refuse_other_request(&table, &replay)?;
            config.extend(rows);
            if let Some(change) = change {
                append(&mut changes, change);
            }
            put(&mut table, replay);
            Ok(())
        }
        .await;
        self.written.after(Written::Settings, result)
    }

    async fn apply_portal_recorded(
        &self,
        user_id: &str,
        edit: PortalEdit,
        record: PortalRecord,
        replay: Option<StoredReplay>,
    ) -> Result<PortalWrite, StoreError> {
        if let Some(replay) = &replay {
            replay.check()?;
        }
        // As SQLite's `to_iso`: a record outside years 1..=9999 is refused
        // before anything moves.
        crate::domain::time::to_iso(&record.at)
            .map_err(|error| StoreError::Constraint(format!("{:?}: {error}", record.at)))?;
        let mut members = self.members();
        let mut changes = self.changes();
        let mut table = self.replays();
        if let Some(replay) = &replay {
            let key = (replay.scope, replay.actor.clone(), replay.key.clone());
            if let Some(answer) = replayed(table.get(&key), replay) {
                return Ok(answer);
            }
        }
        let Some((before, after)) = portal(&mut members, user_id, edit)? else {
            return Ok(PortalWrite::Missing);
        };
        let change = record.change(&before, &after);
        let recorded = change.is_some();
        if let Some(change) = change {
            append(&mut changes, change);
        }
        if let Some(replay) = replay {
            put(&mut table, replay);
        }
        drop((members, changes, table));
        self.written.notify(Written::Members);
        if recorded {
            self.written.notify(Written::Settings);
        }
        Ok(PortalWrite::Edited(after))
    }

    async fn apply_character_replayed(
        &self,
        change: CharacterChange,
        record: CharacterRecord,
        replay: StoredReplay,
    ) -> Result<KeyedWrite<CharacterWrite>, StoreError> {
        self.keyed_character_write(replay, |characters| {
            let (written, change) = super::characters::apply(characters, change, &record)?;
            Ok((written.applied(), written, change))
        })
    }

    async fn apply_pick_replayed(
        &self,
        change: PickChange,
        record: CharacterRecord,
        replay: StoredReplay,
    ) -> Result<KeyedWrite<PickWrite>, StoreError> {
        self.keyed_character_write(replay, |characters| {
            let (written, change) = super::characters::apply_pick(characters, change, &record)?;
            Ok((written.applied(), written, change))
        })
    }

    async fn prune_replays(&self, now: DateTime<Utc>) -> Result<u64, StoreError> {
        Ok(prune(&mut self.replays(), now))
    }
}

impl super::MemoryScheduleStore {
    /// A keyed character-table write under the characters, settings-changes
    /// and replays locks: the key's live answer, or `apply`'s write with its
    /// History row, the replay stored only when the write applied.
    fn keyed_character_write<W>(
        &self,
        replay: StoredReplay,
        apply: impl FnOnce(
            &mut super::characters::CharacterTable,
        ) -> Result<(bool, W, Option<SettingsChange>), StoreError>,
    ) -> Result<KeyedWrite<W>, StoreError> {
        replay.check()?;
        let mut characters = self.characters();
        let mut changes = self.changes();
        let mut table = self.replays();
        let key = (replay.scope, replay.actor.clone(), replay.key.clone());
        if let Some(answer) = keyed(table.get(&key).cloned(), &replay) {
            return Ok(answer);
        }
        let (applied, written, change) = apply(&mut characters)?;
        let recorded = change.is_some();
        if let Some(change) = change {
            append(&mut changes, change);
        }
        if applied {
            put(&mut table, replay);
        }
        drop((characters, changes, table));
        if recorded {
            self.written.notify(Written::Settings);
        }
        Ok(KeyedWrite::Applied(written))
    }
}
