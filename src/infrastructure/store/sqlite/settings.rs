//! `SettingsStore` over the `config` table (0001) and the append-only
//! `settings_changes` (0023): constant SQL, reads on a reader connection,
//! one `BEGIN IMMEDIATE` per write. Settings are not schedule rows, so
//! writes leave the store revision alone.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use sqlx::{Connection, Row, SqliteConnection};

use super::SqliteStore;
use super::rows::{instant, optional_instant};
use super::schedule::store_error;
use crate::domain::history::{Actor, Surface};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::{RowDiff, SettingsChange, SettingsChangeQuery, SettingsStore, keys};
use crate::domain::time::from_iso;

fn key_list() -> String {
    serde_json::Value::from(keys::ALL.to_vec()).to_string()
}

async fn read(
    conn: &mut SqliteConnection,
) -> Result<std::collections::BTreeMap<String, String>, StoreError> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM config WHERE key IN (SELECT value FROM json_each(?1))",
    )
    .bind(key_list())
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(rows.into_iter().collect())
}

async fn write(conn: &mut SqliteConnection, rows: &[(String, String)]) -> Result<(), StoreError> {
    for (key, value) in rows {
        sqlx::query(
            "INSERT INTO config (key, value) VALUES (?1, ?2) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
    }
    Ok(())
}

fn refuse_unknown(rows: &[(String, String)]) -> Result<(), StoreError> {
    match rows.iter().find(|(key, _)| !keys::is_setting(key)) {
        Some((key, _)) => Err(StoreError::Constraint(format!("{key:?} is not a setting"))),
        None => Ok(()),
    }
}

fn encode_values(values: &BTreeMap<String, RowDiff>) -> String {
    let map: Map<String, Value> = values
        .iter()
        .map(|(key, row)| (key.clone(), json!({"from": row.from, "to": row.to})))
        .collect();
    Value::Object(map).to_string()
}

fn corrupt(detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("stored settings change is unreadable: {detail}"))
}

fn decode_values(text: &str) -> Result<BTreeMap<String, RowDiff>, StoreError> {
    let Value::Object(map) = serde_json::from_str::<Value>(text).map_err(corrupt)? else {
        return Err(corrupt("changes is not an object"));
    };
    map.into_iter()
        .map(|(key, row)| {
            let side = |name: &str| {
                row.get(name)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| corrupt(format!("{key}.{name}")))
            };
            Ok((
                key.clone(),
                RowDiff {
                    from: side("from")?,
                    to: side("to")?,
                },
            ))
        })
        .collect()
}

async fn append(conn: &mut SqliteConnection, change: &SettingsChange) -> Result<u64, StoreError> {
    if change.values.is_empty() {
        return Err(StoreError::Constraint(
            "a settings change names at least one row".into(),
        ));
    }
    let revision = i64::try_from(change.revision)
        .map_err(|_| StoreError::Constraint("settings revision out of range".into()))?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO settings_changes \
         (at, actor_kind, actor_id, surface, section, revision, changes) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING id",
    )
    .bind(instant(&change.at)?)
    .bind(change.actor.kind())
    .bind(change.actor.id())
    .bind(change.surface.as_str())
    .bind(&change.section)
    .bind(revision)
    .bind(encode_values(&change.values))
    .fetch_one(&mut *conn)
    .await
    .map_err(store_error)?;
    u64::try_from(id).map_err(corrupt)
}

async fn list(
    conn: &mut SqliteConnection,
    query: &SettingsChangeQuery,
) -> Result<Vec<SettingsChange>, StoreError> {
    let rows = sqlx::query(
        "SELECT id, at, actor_kind, actor_id, surface, section, revision, changes \
         FROM settings_changes \
         WHERE (?1 IS NULL OR (actor_kind = ?1 AND actor_id = ?2)) \
           AND (?3 IS NULL OR at >= ?3) AND (?4 IS NULL OR at < ?4) \
         ORDER BY at DESC, id DESC",
    )
    .bind(query.actor.as_ref().map(Actor::kind))
    .bind(query.actor.as_ref().map(Actor::id))
    .bind(optional_instant(query.from.as_ref())?)
    .bind(optional_instant(query.until.as_ref())?)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_error)?;
    rows.iter()
        .map(|row| {
            let text = |column: &str| row.try_get::<String, _>(column).map_err(corrupt);
            let kind = text("actor_kind")?;
            let actor_id = text("actor_id")?;
            let surface = text("surface")?;
            Ok(SettingsChange {
                id: u64::try_from(row.try_get::<i64, _>("id").map_err(corrupt)?)
                    .map_err(corrupt)?,
                at: from_iso(&text("at")?).map_err(corrupt)?,
                actor: Actor::from_parts(&kind, &actor_id)
                    .ok_or_else(|| corrupt(format!("actor kind {kind}")))?,
                surface: Surface::parse(&surface)
                    .ok_or_else(|| corrupt(format!("surface {surface}")))?,
                section: text("section")?,
                revision: u64::try_from(row.try_get::<i64, _>("revision").map_err(corrupt)?)
                    .map_err(corrupt)?,
                values: decode_values(&text("changes")?)?,
            })
        })
        .collect()
}

impl SettingsStore for SqliteStore {
    async fn settings_rows(
        &self,
    ) -> Result<std::collections::BTreeMap<String, String>, StoreError> {
        read_txn!(self, tx, read(&mut tx))
    }

    async fn put_settings_rows(&self, rows: Vec<(String, String)>) -> Result<(), StoreError> {
        refuse_unknown(&rows)?;
        write_txn!(self, tx, write(&mut tx, &rows))
    }

    async fn put_settings_rows_recorded(
        &self,
        rows: Vec<(String, String)>,
        change: SettingsChange,
    ) -> Result<u64, StoreError> {
        refuse_unknown(&rows)?;
        write_txn!(self, tx, async {
            write(&mut tx, &rows).await?;
            append(&mut tx, &change).await
        })
    }

    async fn settings_changes(
        &self,
        query: SettingsChangeQuery,
    ) -> Result<Vec<SettingsChange>, StoreError> {
        read_txn!(self, tx, list(&mut tx, &query))
    }
}
