//! Ordered, embedded, checksummed schema migrations.
//!
//! All pending migrations apply in one `BEGIN IMMEDIATE` transaction. Each
//! applied version keeps the SHA-256 of its SQL; a changed or unknown applied
//! migration refuses to open rather than guessing.

use ring::digest::{SHA256, digest};
use sqlx::{Connection, Executor, Row, SqliteConnection};

use super::SqliteStoreError;

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
        sql: include_str!("migrations/0002_card_runs.sql"),
    },
    Migration {
        version: 3,
        sql: include_str!("migrations/0003_change_log.sql"),
    },
    Migration {
        version: 4,
        sql: include_str!("migrations/0004_blame_checkpoints.sql"),
    },
    Migration {
        version: 5,
        sql: include_str!("migrations/0005_drafts.sql"),
    },
    Migration {
        version: 6,
        sql: include_str!("migrations/0006_attendance.sql"),
    },
    Migration {
        version: 7,
        sql: include_str!("migrations/0007_model_logs.sql"),
    },
    Migration {
        version: 8,
        sql: include_str!("migrations/0008_log_retention.sql"),
    },
    Migration {
        version: 9,
        sql: include_str!("migrations/0009_web_sessions.sql"),
    },
    Migration {
        version: 10,
        sql: include_str!("migrations/0010_member_profile.sql"),
    },
    Migration {
        version: 11,
        sql: include_str!("migrations/0011_proposal_cards.sql"),
    },
    Migration {
        version: 12,
        sql: include_str!("migrations/0012_notice_outbox.sql"),
    },
    Migration {
        version: 13,
        sql: include_str!("migrations/0013_notice_outbox_retention.sql"),
    },
];

/// The migration that adds `change_fields`, which is backfilled from the
/// records already stored.
const BLAME_INDEX_VERSION: i64 = 4;

const LEDGER: &str = "CREATE TABLE IF NOT EXISTS schema_migrations (
    version    INTEGER PRIMARY KEY,
    checksum   TEXT NOT NULL,
    applied_at TEXT NOT NULL
)";

fn checksum(sql: &str) -> String {
    digest(&SHA256, sql.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Check an applied ledger against the embedded migrations; returns how many
/// are applied.
pub(super) async fn verify(conn: &mut SqliteConnection) -> Result<usize, SqliteStoreError> {
    let applied = sqlx::query("SELECT version, checksum FROM schema_migrations ORDER BY version")
        .fetch_all(&mut *conn)
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
    Ok(applied.len())
}

/// Bring the schema to the newest embedded version; returns that version.
pub(super) async fn apply(conn: &mut SqliteConnection) -> Result<i64, SqliteStoreError> {
    let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
    (&mut *tx).execute(LEDGER).await?;
    let applied = verify(&mut tx).await?;
    for migration in &MIGRATIONS[applied..] {
        (&mut *tx).execute(migration.sql).await?;
        if migration.version == BLAME_INDEX_VERSION {
            // Index the records written before the blame index existed.
            super::history::backfill_fields(&mut tx)
                .await
                .map_err(|error| {
                    SqliteStoreError::Database(sqlx::Error::Protocol(error.to_string()))
                })?;
        }
        sqlx::query(
            "INSERT INTO schema_migrations (version, checksum, applied_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now'))",
        )
        .bind(migration.version)
        .bind(checksum(migration.sql))
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE store_meta SET schema_version = ?1 WHERE id = 1")
            .bind(migration.version)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(MIGRATIONS.last().map_or(0, |last| last.version))
}
