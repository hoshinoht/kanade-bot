//! In-memory `ReplayStore`, mirroring SQLite's `idempotency_replays`: a
//! write first drops the rows expired at its `created_at`, and a live row of
//! another request refuses.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::settings::{append, refuse_change, refuse_unknown};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::SettingsChange;
use crate::infrastructure::store::Written;
use crate::infrastructure::store::replays::{ReplayScope, ReplayStore, StoredReplay};

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
            refuse_unknown(&rows)?;
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

    async fn prune_replays(&self, now: DateTime<Utc>) -> Result<u64, StoreError> {
        Ok(prune(&mut self.replays(), now))
    }
}
