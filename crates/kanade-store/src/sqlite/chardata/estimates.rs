//! Scouter estimates in the character-data file (`0003`): each
//! character's latest result and each party's latest total with the
//! characters it includes (cascading with the total). Results are stored as
//! JSON text in kanade's own shape (`domain::chardata::StoredMember`).

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Connection;

use super::super::rows::list;
use super::super::schedule::store_error;
use super::rows::{member_estimate, party_estimate};
use super::{SqliteCharData, instant};
use kanade_domain::chardata::{
    DeletedEstimates, ESTIMATE_TTL, FUTURE_SKEW, MemberEstimate, PartyEstimate, StoredEstimates,
};
use kanade_domain::scheduler::StoreError;

fn encode(value: &impl Serialize) -> Result<String, StoreError> {
    serde_json::to_string(value)
        .map_err(|_| StoreError::Constraint("estimate result does not encode".into()))
}

/// The bounds of [`kanade_domain::chardata::estimate_kept`] at `now`: kept
/// rows are stamped after the first and at or before the second.
fn window(now: DateTime<Utc>) -> Result<(String, String), StoreError> {
    Ok((
        instant(&(now - ESTIMATE_TTL))?,
        instant(&(now + FUTURE_SKEW))?,
    ))
}

impl SqliteCharData {
    pub(super) async fn load_estimates(
        &self,
        now: DateTime<Utc>,
        limit: u32,
    ) -> Result<StoredEstimates, StoreError> {
        let (after, until) = window(now)?;
        write_txn!(self, tx, async {
            let members = sqlx::query(
                "SELECT character_id, ign_key, result, fetched_at FROM estimate_members
                 WHERE fetched_at > ?1 AND fetched_at <= ?2
                 ORDER BY fetched_at DESC, character_id LIMIT ?3",
            )
            .bind(&after)
            .bind(&until)
            .bind(i64::from(limit))
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?
            .iter()
            .map(member_estimate)
            .collect::<Result<_, _>>()?;
            let parties = sqlx::query(
                "SELECT p.set_key, p.result, p.fetched_at,
                     (SELECT json_group_array(m.character_id) FROM estimate_party_members m
                      WHERE m.set_key = p.set_key) AS character_ids
                 FROM estimate_parties p
                 WHERE p.fetched_at > ?1 AND p.fetched_at <= ?2
                 ORDER BY p.fetched_at DESC, p.set_key LIMIT ?3",
            )
            .bind(&after)
            .bind(&until)
            .bind(i64::from(limit))
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?
            .iter()
            .map(party_estimate)
            .collect::<Result<_, _>>()?;
            Ok(StoredEstimates { members, parties })
        })
    }

    pub(super) async fn write_member_estimate(
        &self,
        new: MemberEstimate,
    ) -> Result<(), StoreError> {
        new.check()?;
        let (result, fetched_at) = (encode(&new.result)?, instant(&new.fetched_at)?);
        write_txn!(self, tx, async {
            sqlx::query(
                "INSERT INTO estimate_members (character_id, ign_key, result, fetched_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (character_id) DO UPDATE SET ign_key = excluded.ign_key,
                     result = excluded.result, fetched_at = excluded.fetched_at",
            )
            .bind(&new.character_id)
            .bind(&new.ign_key)
            .bind(&result)
            .bind(&fetched_at)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            Ok(())
        })
    }

    pub(super) async fn write_party_estimate(&self, new: PartyEstimate) -> Result<(), StoreError> {
        new.check()?;
        let (result, fetched_at) = (encode(&new.combined)?, instant(&new.fetched_at)?);
        let characters = list(&new.character_ids);
        write_txn!(self, tx, async {
            // An upsert, never REPLACE: the member rows are rewritten below.
            sqlx::query(
                "INSERT INTO estimate_parties (set_key, result, fetched_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (set_key) DO UPDATE SET result = excluded.result,
                     fetched_at = excluded.fetched_at",
            )
            .bind(&new.set_key)
            .bind(&result)
            .bind(&fetched_at)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            sqlx::query("DELETE FROM estimate_party_members WHERE set_key = ?1")
                .bind(&new.set_key)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
            sqlx::query(
                "INSERT INTO estimate_party_members (set_key, character_id)
                 SELECT ?1, value FROM json_each(?2)",
            )
            .bind(&new.set_key)
            .bind(&characters)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            Ok(())
        })
    }

    pub(super) async fn remove_estimates(
        &self,
        character_id: String,
    ) -> Result<DeletedEstimates, StoreError> {
        write_txn!(self, tx, async {
            let ign_key: Option<String> =
                sqlx::query_scalar("SELECT ign_key FROM estimate_members WHERE character_id = ?1")
                    .bind(&character_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(store_error)?;
            let set_keys: Vec<String> = sqlx::query_scalar(
                "SELECT set_key FROM estimate_party_members WHERE character_id = ?1
                 ORDER BY set_key",
            )
            .bind(&character_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_error)?;
            sqlx::query("DELETE FROM estimate_members WHERE character_id = ?1")
                .bind(&character_id)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
            // Their member rows cascade with the totals.
            sqlx::query(
                "DELETE FROM estimate_parties WHERE set_key IN (SELECT value FROM json_each(?1))",
            )
            .bind(list(&set_keys))
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            Ok(DeletedEstimates { ign_key, set_keys })
        })
    }

    pub(super) async fn sweep_estimate_rows(&self, now: DateTime<Utc>) -> Result<u64, StoreError> {
        let (after, until) = window(now)?;
        write_txn!(self, tx, async {
            let mut done = 0;
            for sql in [
                "DELETE FROM estimate_members WHERE fetched_at <= ?1 OR fetched_at > ?2",
                "DELETE FROM estimate_parties WHERE fetched_at <= ?1 OR fetched_at > ?2",
            ] {
                done += sqlx::query(sql)
                    .bind(&after)
                    .bind(&until)
                    .execute(&mut *tx)
                    .await
                    .map_err(store_error)?
                    .rows_affected();
            }
            Ok(done)
        })
    }
}
