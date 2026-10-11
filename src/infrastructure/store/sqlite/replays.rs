//! `ReplayStore` over `idempotency_replays` (0031): constant SQL, reads on a
//! reader connection, one `BEGIN IMMEDIATE` per write, which first deletes
//! the rows already expired.

use chrono::{DateTime, Utc};
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use super::rows::instant;
use super::schedule::store_error;
use super::{characters, members, settings};
use crate::domain::characters::{CharacterChange, CharacterRecord, CharacterWrite};
use crate::domain::members::PortalEdit;
use crate::domain::scheduler::StoreError;
use crate::domain::settings::SettingsChange;
use crate::infrastructure::store::Written;
use crate::infrastructure::store::replays::{
    KeyedWrite, PortalRecord, PortalWrite, ReplayScope, ReplayStore, StoredReplay, keyed, replayed,
};

fn corrupt(column: &str, detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!(
        "idempotency_replays.{column} is unreadable: {detail}"
    ))
}

async fn read(
    conn: &mut SqliteConnection,
    scope: ReplayScope,
    actor: &str,
    key: &str,
) -> Result<Option<StoredReplay>, StoreError> {
    let Some(row) = sqlx::query(
        "SELECT request, status, body, created_at, expires_at FROM idempotency_replays \
         WHERE scope = ?1 AND actor = ?2 AND key = ?3",
    )
    .bind(scope.as_str())
    .bind(actor)
    .bind(key)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?
    else {
        return Ok(None);
    };
    let text = |column: &str| {
        row.try_get::<String, _>(column)
            .map_err(|error| corrupt(column, error))
    };
    let at = |column: &str| {
        crate::domain::time::from_iso(&text(column)?).map_err(|error| corrupt(column, error))
    };
    let status: i64 = row
        .try_get("status")
        .map_err(|error| corrupt("status", error))?;
    Ok(Some(StoredReplay {
        scope,
        actor: actor.to_owned(),
        key: key.to_owned(),
        request: text("request")?,
        status: u16::try_from(status).map_err(|error| corrupt("status", error))?,
        body: text("body")?,
        created_at: at("created_at")?,
        expires_at: at("expires_at")?,
    }))
}

async fn prune(conn: &mut SqliteConnection, now: &DateTime<Utc>) -> Result<u64, StoreError> {
    Ok(
        sqlx::query("DELETE FROM idempotency_replays WHERE expires_at <= ?1")
            .bind(instant(now)?)
            .execute(&mut *conn)
            .await
            .map_err(store_error)?
            .rows_affected(),
    )
}

/// Drop expired rows, then insert `replay` or update the live row of the
/// same request; a live row of another request refuses.
async fn put(conn: &mut SqliteConnection, replay: &StoredReplay) -> Result<(), StoreError> {
    prune(conn, &replay.created_at).await?;
    let written = sqlx::query(
        "INSERT INTO idempotency_replays \
         (scope, actor, key, request, status, body, created_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT (scope, actor, key) DO UPDATE SET \
         status = excluded.status, body = excluded.body, \
         created_at = excluded.created_at, expires_at = excluded.expires_at \
         WHERE idempotency_replays.request = excluded.request",
    )
    .bind(replay.scope.as_str())
    .bind(&replay.actor)
    .bind(&replay.key)
    .bind(&replay.request)
    .bind(i64::from(replay.status))
    .bind(&replay.body)
    .bind(instant(&replay.created_at)?)
    .bind(instant(&replay.expires_at)?)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?
    .rows_affected();
    if written == 0 {
        return Err(StoreError::Constraint(
            "replay key holds another live request".into(),
        ));
    }
    Ok(())
}

impl ReplayStore for SqliteStore {
    async fn replay(
        &self,
        scope: ReplayScope,
        actor: &str,
        key: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<StoredReplay>, StoreError> {
        let found = read_txn!(self, tx, read(&mut tx, scope, actor, key))?;
        Ok(found.filter(|replay| replay.live(now)))
    }

    async fn put_replay(&self, replay: StoredReplay) -> Result<(), StoreError> {
        replay.check()?;
        write_txn!(self, tx, put(&mut tx, &replay))
    }

    async fn put_settings_rows_replayed(
        &self,
        rows: Vec<(String, String)>,
        change: Option<SettingsChange>,
        replay: StoredReplay,
    ) -> Result<(), StoreError> {
        settings::refuse_unknown(&rows)?;
        replay.check()?;
        let result = write_txn!(self, tx, async {
            settings::write(&mut tx, &rows).await?;
            if let Some(change) = &change {
                settings::append(&mut tx, change).await?;
            }
            put(&mut tx, &replay).await
        });
        self.written().after(Written::Settings, result)
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
        // The key is checked inside the write, so a concurrent retry waits
        // for the first request and then finds its answer.
        let result = write_txn!(self, tx, async {
            if let Some(replay) = &replay {
                let found = read(&mut tx, replay.scope, &replay.actor, &replay.key).await?;
                if let Some(answer) = replayed(found.as_ref(), replay) {
                    return Ok((answer, false));
                }
            }
            let Some((before, after)) = members::portal(&mut tx, user_id, &edit).await? else {
                return Ok((PortalWrite::Missing, false));
            };
            let change = record.change(&before, &after);
            if let Some(change) = &change {
                settings::append(&mut tx, change).await?;
            }
            if let Some(replay) = &replay {
                put(&mut tx, replay).await?;
            }
            Ok((PortalWrite::Edited(after), change.is_some()))
        });
        let (written, recorded) = result?;
        if matches!(written, PortalWrite::Edited(_)) {
            self.written().notify(Written::Members);
        }
        if recorded {
            self.written().notify(Written::Settings);
        }
        Ok(written)
    }

    async fn apply_character_replayed(
        &self,
        change: CharacterChange,
        record: CharacterRecord,
        replay: StoredReplay,
    ) -> Result<KeyedWrite<CharacterWrite>, StoreError> {
        replay.check()?;
        // The key is checked inside the write, so a concurrent retry waits
        // for the first request and then finds its answer.
        let (written, recorded) = write_txn!(self, tx, async {
            let found = read(&mut tx, replay.scope, &replay.actor, &replay.key).await?;
            if let Some(answer) = keyed(found, &replay) {
                return Ok((answer, false));
            }
            let (written, recorded) = characters::apply(&mut tx, change, &record).await?;
            if written.applied() {
                put(&mut tx, &replay).await?;
            }
            Ok((KeyedWrite::Applied(written), recorded))
        })?;
        if recorded {
            self.written().notify(Written::Settings);
        }
        Ok(written)
    }

    async fn prune_replays(&self, now: DateTime<Utc>) -> Result<u64, StoreError> {
        write_txn!(self, tx, prune(&mut tx, &now))
    }
}
