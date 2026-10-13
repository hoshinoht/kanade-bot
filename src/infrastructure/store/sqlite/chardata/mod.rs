//! The character-data file ([`SqliteCharData`]): a second SQLite database on
//! its own volume, owned like the main store (same owner lock directory,
//! lock named by its inode), with its own ledger (`migrate.rs`). One
//! connection serves every call, each its own `BEGIN IMMEDIATE`; the
//! connection keeps a rollback journal deleted after each transaction and
//! `secure_delete=ON`, so deleted Nexon data is overwritten in place. All
//! SQL is constant. `kanade backup`/`restore` never open this file.

mod migrate;
mod rows;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use sqlx::Connection;

use super::backup::AbandonOnDrop;
use super::owner::StoreOwner;
use super::schedule::store_error;
use super::writer::{LeaseError, Writer, WriterLease};
use super::{SqliteStoreConfig, SqliteStoreError, connect};
use crate::domain::chardata::{
    CALL_WINDOW, CharDataStore, ConsentSurface, FUTURE_SKEW, Link, LinkPut, LinkStatus, Post,
    PostState, RefreshState, Snapshot, StoreFuture, date_text,
};
use crate::domain::scheduler::StoreError;
use rows::{LINK_COLUMNS, POST_COLUMNS, SNAPSHOT_COLUMNS, at, link, post, snapshot};

/// The owning handle on the character-data file. Call [`Self::close`].
pub struct SqliteCharData {
    writer: Writer,
    // Declared last: SQLite handles drop before ownership is released.
    _owner: StoreOwner,
}

impl SqliteCharData {
    /// Take ownership (creating the file 0600 if absent), then create or
    /// migrate its schema. Runs as its own task, as [`super::SqliteStore::open`].
    ///
    /// # Errors
    /// Unsafe paths, another owner, or a ledger refusal.
    pub async fn open(config: &SqliteStoreConfig) -> Result<Self, SqliteStoreError> {
        let config = config.clone();
        let abandoned = Arc::new(AtomicBool::new(false));
        let _abandon_on_drop = AbandonOnDrop(Arc::clone(&abandoned));
        let task = tokio::spawn(async move {
            let owner = StoreOwner::acquire(&config)?;
            owner.identity().verify()?;
            let mut conn = connect::chardata(&config.db_path).await?;
            if let Err(error) = migrate::apply(&mut conn).await {
                let _ = conn.close().await;
                return Err(error);
            }
            let store = Self {
                writer: Writer::with_profile(
                    config.db_path.clone(),
                    owner.identity(),
                    conn,
                    connect::Profile::CharData,
                ),
                _owner: owner,
            };
            if abandoned.load(Ordering::SeqCst) {
                let _ = store.close().await;
                return Err(SqliteStoreError::Io {
                    path: config.db_path.clone(),
                    source: std::io::Error::other("open abandoned by its caller"),
                });
            }
            Ok(store)
        });
        task.await.map_err(|error| SqliteStoreError::Io {
            path: PathBuf::from("open task"),
            source: std::io::Error::other(error),
        })?
    }

    /// Close the connection (and any a cancelled call orphaned), then
    /// release ownership.
    ///
    /// # Errors
    /// Closing failed; ownership is released regardless.
    pub async fn close(self) -> Result<(), SqliteStoreError> {
        let Self { writer, _owner } = self;
        writer.shutdown().await.map_err(Into::into)
    }

    async fn writer_lease(&self) -> Result<WriterLease<'_>, LeaseError> {
        self.writer.lease().await
    }
}

fn instant(at: &DateTime<Utc>) -> Result<String, StoreError> {
    crate::domain::time::to_iso(at).map_err(|error| StoreError::Constraint(error.to_string()))
}

impl SqliteCharData {
    async fn link_row(&self, character_id: String) -> Result<Option<Link>, StoreError> {
        write_txn!(self, tx, async {
            sqlx::query(&format!(
                "SELECT {LINK_COLUMNS} FROM links WHERE character_id = ?1"
            ))
            .bind(&character_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_error)?
            .as_ref()
            .map(link)
            .transpose()
        })
    }

    async fn link_rows(&self) -> Result<Vec<Link>, StoreError> {
        write_txn!(self, tx, async {
            sqlx::query(&format!(
                "SELECT {LINK_COLUMNS} FROM links ORDER BY character_id"
            ))
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?
            .iter()
            .map(link)
            .collect()
        })
    }

    async fn write_link(&self, new: Link, expected: u32) -> Result<LinkPut, StoreError> {
        new.check()?;
        if new.version != expected + 1 {
            return Err(StoreError::Constraint("link takes the next version".into()));
        }
        let requested_at = instant(&new.requested_at)?;
        let consented_at = new.consented_at.as_ref().map(instant).transpose()?;
        let linked_at = new.linked_at.as_ref().map(instant).transpose()?;
        write_txn!(self, tx, async {
            let stored = sqlx::query(&format!(
                "SELECT {LINK_COLUMNS} FROM links WHERE character_id = ?1"
            ))
            .bind(&new.character_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_error)?
            .as_ref()
            .map(link)
            .transpose()?;
            if stored.as_ref().map_or(0, |stored| stored.version) != expected {
                return Ok(LinkPut::Stale(stored));
            }
            // An upsert, never REPLACE: a delete would cascade the snapshot.
            sqlx::query(
                "INSERT INTO links (character_id, member_id, ign_key, region, status,
                     requested_by_kind, requested_by_id, requested_at, consent_version,
                     consented_at, linked_at, version, consent_surface)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT (character_id) DO UPDATE SET member_id = excluded.member_id,
                     ign_key = excluded.ign_key, region = excluded.region,
                     status = excluded.status, requested_by_kind = excluded.requested_by_kind,
                     requested_by_id = excluded.requested_by_id,
                     requested_at = excluded.requested_at,
                     consent_version = excluded.consent_version,
                     consented_at = excluded.consented_at, linked_at = excluded.linked_at,
                     version = excluded.version, consent_surface = excluded.consent_surface",
            )
            .bind(&new.character_id)
            .bind(&new.member_id)
            .bind(&new.ign_key)
            .bind(&new.region)
            .bind(new.status.as_str())
            .bind(new.requested_by.kind())
            .bind(new.requested_by.id())
            .bind(&requested_at)
            .bind(new.consent_version.map(i64::from))
            .bind(&consented_at)
            .bind(&linked_at)
            .bind(i64::from(new.version))
            .bind(new.consent_surface.map(ConsentSurface::as_str))
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            if new.status != LinkStatus::Active {
                for sql in [
                    "DELETE FROM snapshots WHERE character_id = ?1",
                    "DELETE FROM refresh_state WHERE character_id = ?1",
                ] {
                    sqlx::query(sql)
                        .bind(&new.character_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(store_error)?;
                }
            }
            Ok(LinkPut::Written)
        })
    }

    async fn delete_link(
        &self,
        character_id: String,
        now: DateTime<Utc>,
    ) -> Result<Option<Link>, StoreError> {
        let now = instant(&now)?;
        write_txn!(self, tx, async {
            let stored = sqlx::query(&format!(
                "SELECT {LINK_COLUMNS} FROM links WHERE character_id = ?1"
            ))
            .bind(&character_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_error)?
            .as_ref()
            .map(link)
            .transpose()?;
            // The snapshot and refresh state cascade with the link.
            sqlx::query("DELETE FROM links WHERE character_id = ?1")
                .bind(&character_id)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
            sqlx::query(
                "UPDATE posts SET strip_due_at = ?2
                 WHERE character_id = ?1 AND strip_due_at > ?2",
            )
            .bind(&character_id)
            .bind(&now)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            Ok(stored)
        })
    }

    async fn fresh_snapshot(
        &self,
        character_id: String,
        now: DateTime<Utc>,
    ) -> Result<Option<Snapshot>, StoreError> {
        let found = write_txn!(self, tx, async {
            sqlx::query(&format!(
                "SELECT {SNAPSHOT_COLUMNS} FROM snapshots WHERE character_id = ?1"
            ))
            .bind(&character_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_error)?
            .as_ref()
            .map(snapshot)
            .transpose()
        })?;
        // Checked on the decoded instants, as the memory store does.
        Ok(found.filter(|snapshot| snapshot.is_fresh(now)))
    }

    async fn write_snapshot(&self, new: Snapshot) -> Result<bool, StoreError> {
        new.check()?;
        let (ocid_at, fetched_at, expires_at) = (
            instant(&new.ocid_fetched_at)?,
            instant(&new.fetched_at)?,
            instant(&new.expires_at)?,
        );
        let date = new.data_date.map(date_text);
        write_txn!(self, tx, async {
            let written = sqlx::query(
                "INSERT INTO snapshots (character_id, ocid, ocid_fetched_at, character_name,
                     world_name, character_class, level, combat_power, image_url, data_date,
                     fetched_at, expires_at)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
                 WHERE EXISTS (SELECT 1 FROM links WHERE character_id = ?1 AND status = 'active')
                 ON CONFLICT (character_id) DO UPDATE SET ocid = excluded.ocid,
                     ocid_fetched_at = excluded.ocid_fetched_at,
                     character_name = excluded.character_name,
                     world_name = excluded.world_name,
                     character_class = excluded.character_class, level = excluded.level,
                     combat_power = excluded.combat_power, image_url = excluded.image_url,
                     data_date = excluded.data_date, fetched_at = excluded.fetched_at,
                     expires_at = excluded.expires_at",
            )
            .bind(&new.character_id)
            .bind(&new.ocid)
            .bind(&ocid_at)
            .bind(&new.character_name)
            .bind(&new.world_name)
            .bind(&new.character_class)
            .bind(new.level.map(i64::from))
            .bind(
                new.combat_power
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| StoreError::Constraint("combat power is too large".into()))?,
            )
            .bind(&new.image_url)
            .bind(&date)
            .bind(&fetched_at)
            .bind(&expires_at)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            Ok(written.rows_affected() > 0)
        })
    }

    async fn sweep(&self, now: DateTime<Utc>) -> Result<u64, StoreError> {
        let (now_text, ahead) = (instant(&now)?, instant(&(now + FUTURE_SKEW))?);
        write_txn!(self, tx, async {
            let done = sqlx::query(
                "DELETE FROM snapshots
                 WHERE expires_at <= ?1 OR fetched_at > ?2 OR ocid_fetched_at > ?2",
            )
            .bind(&now_text)
            .bind(&ahead)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            Ok(done.rows_affected())
        })
    }

    async fn refresh_rows(&self) -> Result<Vec<RefreshState>, StoreError> {
        write_txn!(self, tx, async {
            let rows = sqlx::query(
                "SELECT character_id, next_attempt_at, last_error_code FROM refresh_state
                 ORDER BY character_id",
            )
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?;
            rows.iter()
                .map(|row| {
                    use sqlx::Row;
                    let next: Option<String> =
                        row.try_get("next_attempt_at").map_err(store_error)?;
                    Ok(RefreshState {
                        character_id: row.try_get("character_id").map_err(store_error)?,
                        next_attempt_at: next.as_deref().map(at).transpose()?,
                        last_error_code: row.try_get("last_error_code").map_err(store_error)?,
                    })
                })
                .collect()
        })
    }

    async fn write_refresh(&self, state: RefreshState) -> Result<bool, StoreError> {
        let next = state.next_attempt_at.as_ref().map(instant).transpose()?;
        write_txn!(self, tx, async {
            let done = sqlx::query(
                "INSERT INTO refresh_state (character_id, next_attempt_at, last_error_code)
                 SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM links WHERE character_id = ?1)
                 ON CONFLICT (character_id) DO UPDATE SET
                     next_attempt_at = excluded.next_attempt_at,
                     last_error_code = excluded.last_error_code",
            )
            .bind(&state.character_id)
            .bind(&next)
            .bind(&state.last_error_code)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            Ok(done.rows_affected() > 0)
        })
    }

    async fn reserve(&self, now: DateTime<Utc>, cap: u32) -> Result<bool, StoreError> {
        let (now_text, since) = (instant(&now)?, instant(&(now - CALL_WINDOW))?);
        write_txn!(self, tx, async {
            let used: i64 = sqlx::query_scalar("SELECT count(*) FROM calls WHERE at > ?1")
                .bind(&since)
                .fetch_one(&mut *tx)
                .await
                .map_err(store_error)?;
            if used >= i64::from(cap) {
                return Ok(false);
            }
            sqlx::query("INSERT INTO calls (at) VALUES (?1)")
                .bind(&now_text)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
            Ok(true)
        })
    }

    async fn count_calls(&self, since: DateTime<Utc>) -> Result<u32, StoreError> {
        let since = instant(&since)?;
        let count: i64 = write_txn!(self, tx, async {
            sqlx::query_scalar("SELECT count(*) FROM calls WHERE at > ?1")
                .bind(&since)
                .fetch_one(&mut *tx)
                .await
                .map_err(store_error)
        })?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    async fn drop_calls(&self, before: DateTime<Utc>) -> Result<u64, StoreError> {
        let before = instant(&before)?;
        write_txn!(self, tx, async {
            let done = sqlx::query("DELETE FROM calls WHERE at <= ?1")
                .bind(&before)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
            Ok(done.rows_affected())
        })
    }

    async fn insert_post(&self, new: Post) -> Result<bool, StoreError> {
        new.check()?;
        let (posted, due) = (instant(&new.posted_at)?, instant(&new.strip_due_at)?);
        write_txn!(self, tx, async {
            let done = sqlx::query(
                "INSERT INTO posts (ref, channel_id, message_id, character_id, posted_at,
                     strip_due_at, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT (ref) DO NOTHING",
            )
            .bind(&new.reference)
            .bind(&new.channel_id)
            .bind(&new.message_id)
            .bind(&new.character_id)
            .bind(&posted)
            .bind(&due)
            .bind(new.state.as_str())
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            Ok(done.rows_affected() > 0)
        })
    }

    async fn post_rows(&self) -> Result<Vec<Post>, StoreError> {
        write_txn!(self, tx, async {
            sqlx::query(&format!(
                "SELECT {POST_COLUMNS} FROM posts ORDER BY posted_at, ref"
            ))
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?
            .iter()
            .map(post)
            .collect()
        })
    }

    async fn update_post(
        &self,
        sql: &'static str,
        reference: String,
        value: String,
    ) -> Result<bool, StoreError> {
        write_txn!(self, tx, async {
            let done = sqlx::query(sql)
                .bind(&reference)
                .bind(&value)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
            Ok(done.rows_affected() > 0)
        })
    }
}

impl CharDataStore for SqliteCharData {
    fn link(&self, character_id: String) -> StoreFuture<'_, Option<Link>> {
        Box::pin(self.link_row(character_id))
    }

    fn links(&self) -> StoreFuture<'_, Vec<Link>> {
        Box::pin(self.link_rows())
    }

    fn put_link(&self, link: Link, expected: u32) -> StoreFuture<'_, LinkPut> {
        Box::pin(self.write_link(link, expected))
    }

    fn unlink(&self, character_id: String, at: DateTime<Utc>) -> StoreFuture<'_, Option<Link>> {
        Box::pin(self.delete_link(character_id, at))
    }

    fn snapshot(
        &self,
        character_id: String,
        now: DateTime<Utc>,
    ) -> StoreFuture<'_, Option<Snapshot>> {
        Box::pin(self.fresh_snapshot(character_id, now))
    }

    fn put_snapshot(&self, snapshot: Snapshot) -> StoreFuture<'_, bool> {
        Box::pin(self.write_snapshot(snapshot))
    }

    fn sweep_snapshots(&self, now: DateTime<Utc>) -> StoreFuture<'_, u64> {
        Box::pin(self.sweep(now))
    }

    fn refresh_states(&self) -> StoreFuture<'_, Vec<RefreshState>> {
        Box::pin(self.refresh_rows())
    }

    fn put_refresh_state(&self, state: RefreshState) -> StoreFuture<'_, bool> {
        Box::pin(self.write_refresh(state))
    }

    fn reserve_call(&self, at: DateTime<Utc>, cap: u32) -> StoreFuture<'_, bool> {
        Box::pin(self.reserve(at, cap))
    }

    fn calls_since(&self, since: DateTime<Utc>) -> StoreFuture<'_, u32> {
        Box::pin(self.count_calls(since))
    }

    fn prune_calls(&self, before: DateTime<Utc>) -> StoreFuture<'_, u64> {
        Box::pin(self.drop_calls(before))
    }

    fn create_post(&self, post: Post) -> StoreFuture<'_, bool> {
        Box::pin(self.insert_post(post))
    }

    fn posts(&self) -> StoreFuture<'_, Vec<Post>> {
        Box::pin(self.post_rows())
    }

    fn set_post_message(&self, reference: String, message_id: String) -> StoreFuture<'_, bool> {
        Box::pin(self.update_post(
            "UPDATE posts SET message_id = ?2 WHERE ref = ?1",
            reference,
            message_id,
        ))
    }

    fn set_post_state(&self, reference: String, state: PostState) -> StoreFuture<'_, bool> {
        Box::pin(self.update_post(
            "UPDATE posts SET state = ?2 WHERE ref = ?1",
            reference,
            state.as_str().to_owned(),
        ))
    }

    fn delete_post(&self, reference: String) -> StoreFuture<'_, bool> {
        Box::pin(async move {
            write_txn!(self, tx, async {
                let done = sqlx::query("DELETE FROM posts WHERE ref = ?1")
                    .bind(&reference)
                    .execute(&mut *tx)
                    .await
                    .map_err(store_error)?;
                Ok(done.rows_affected() > 0)
            })
        })
    }
}
