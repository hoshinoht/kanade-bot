//! `SettingsStore` over the `config` table (0001): constant SQL, reads on a
//! reader connection, one `BEGIN IMMEDIATE` per write. Settings are not
//! schedule rows, so writes leave the store revision alone.

use sqlx::{Connection, SqliteConnection};

use super::SqliteStore;
use super::schedule::store_error;
use crate::domain::scheduler::StoreError;
use crate::domain::settings::{SettingsStore, keys};

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

impl SettingsStore for SqliteStore {
    async fn settings_rows(
        &self,
    ) -> Result<std::collections::BTreeMap<String, String>, StoreError> {
        read_txn!(self, tx, read(&mut tx))
    }

    async fn put_settings_rows(&self, rows: Vec<(String, String)>) -> Result<(), StoreError> {
        if let Some((key, _)) = rows.iter().find(|(key, _)| !keys::is_setting(key)) {
            return Err(StoreError::Constraint(format!("{key:?} is not a setting")));
        }
        write_txn!(self, tx, write(&mut tx, &rows))
    }
}
