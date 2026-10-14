//! The character-data file's own migration ledger (`chardata_migrations`),
//! checksummed like the main store's: a changed, unknown or skipped applied
//! version refuses to open. An approved exception to "one ledger": the file
//! lives on its own volume, outside backups.

use sqlx::{Connection, Executor, Row, SqliteConnection};

use super::super::SqliteStoreError;
use super::super::migrate::checksum;

struct Migration {
    version: i64,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("migrations/0002_link_surface.sql"),
    },
];

const LEDGER: &str = "CREATE TABLE IF NOT EXISTS chardata_migrations (
    version    INTEGER PRIMARY KEY,
    checksum   TEXT NOT NULL,
    applied_at TEXT NOT NULL
)";

/// Bring the file to the newest embedded version in one transaction;
/// returns that version.
pub(super) async fn apply(conn: &mut SqliteConnection) -> Result<i64, SqliteStoreError> {
    let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
    (&mut *tx).execute(LEDGER).await?;
    let applied = sqlx::query("SELECT version, checksum FROM chardata_migrations ORDER BY version")
        .fetch_all(&mut *tx)
        .await?;
    let known = MIGRATIONS.last().map_or(0, |last| last.version);
    for (index, row) in applied.iter().enumerate() {
        let version: i64 = row.try_get("version")?;
        let stored: String = row.try_get("checksum")?;
        if version > known {
            return Err(SqliteStoreError::FutureVersion {
                found: version,
                known,
            });
        }
        let migration = &MIGRATIONS[index];
        if migration.version != version {
            return Err(SqliteStoreError::MigrationGap { version });
        }
        if checksum(migration.sql) != stored {
            return Err(SqliteStoreError::ChecksumMismatch { version });
        }
    }
    for migration in &MIGRATIONS[applied.len()..] {
        (&mut *tx).execute(migration.sql).await?;
        sqlx::query(
            "INSERT INTO chardata_migrations (version, checksum, applied_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now'))",
        )
        .bind(migration.version)
        .bind(checksum(migration.sql))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(known)
}

#[cfg(test)]
mod tests {
    use super::{MIGRATIONS, checksum};

    /// Shipped character-data migrations are frozen byte for byte, as the
    /// main store's are: append new versions, never edit old ones.
    #[test]
    fn shipped_chardata_migrations_never_change() {
        const SHIPPED: &[(i64, &str)] = &[
            (
                1,
                "2566d5ba648ca185c4d105864324fc2f2ecf53c0e2c59d41c1b38e40faf22dcb",
            ),
            (
                2,
                "ec706a780435e20f64fd82bbd688bdb0566cc49f37b2811a811b5a3b4546945b",
            ),
        ];
        for (version, sum) in SHIPPED {
            let migration = MIGRATIONS
                .iter()
                .find(|m| m.version == *version)
                .unwrap_or_else(|| panic!("chardata migration {version} is gone"));
            assert_eq!(
                checksum(migration.sql),
                *sum,
                "chardata migration {version} changed; add a new migration instead"
            );
        }
    }
}
