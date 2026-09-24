use crate::model::Delivery;
use sqlx::{PgPool, Row, SqlitePool};

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS delivery_ledger (guild_id BIGINT NOT NULL, run_id TEXT NOT NULL PRIMARY KEY, active_delivery_id TEXT NOT NULL, occurred_at TEXT NOT NULL)";

pub async fn setup_sqlite(url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePool::connect(url).await?;
    sqlx::query("PRAGMA journal_mode = WAL")
        .execute(&pool)
        .await?;
    sqlx::query(SCHEMA).execute(&pool).await?;
    Ok(pool)
}

pub async fn setup_postgres(url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = PgPool::connect(url).await?;
    sqlx::query(SCHEMA).execute(&pool).await?;
    Ok(pool)
}

pub async fn clear_postgres(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("TRUNCATE delivery_ledger")
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn write_sqlite(pool: &SqlitePool, rows: &[Delivery]) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    for row in rows {
        sqlx::query("INSERT INTO delivery_ledger (guild_id, run_id, active_delivery_id, occurred_at) VALUES (?, ?, ?, ?)")
            .bind(row.guild_id).bind(&row.run_id).bind(&row.active_delivery_id).bind(&row.occurred_at).execute(&mut *tx).await?;
    }
    tx.commit().await
}

pub async fn write_postgres(pool: &PgPool, rows: &[Delivery]) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for row in rows {
        sqlx::query("INSERT INTO delivery_ledger (guild_id, run_id, active_delivery_id, occurred_at) VALUES ($1, $2, $3, $4)")
            .bind(row.guild_id).bind(&row.run_id).bind(&row.active_delivery_id).bind(&row.occurred_at).execute(&mut *tx).await?;
    }
    tx.commit().await
}

pub async fn snapshot_sqlite(pool: &SqlitePool) -> Result<Vec<Delivery>, sqlx::Error> {
    let rows = sqlx::query("SELECT guild_id, run_id, active_delivery_id, occurred_at FROM delivery_ledger ORDER BY run_id")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| Delivery {
            guild_id: row.get("guild_id"),
            run_id: row.get("run_id"),
            active_delivery_id: row.get("active_delivery_id"),
            occurred_at: row.get("occurred_at"),
        })
        .collect())
}
pub async fn snapshot_postgres(pool: &PgPool) -> Result<Vec<Delivery>, sqlx::Error> {
    let rows = sqlx::query("SELECT guild_id, run_id, active_delivery_id, occurred_at FROM delivery_ledger ORDER BY run_id")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| Delivery {
            guild_id: row.get("guild_id"),
            run_id: row.get("run_id"),
            active_delivery_id: row.get("active_delivery_id"),
            occurred_at: row.get("occurred_at"),
        })
        .collect())
}

pub async fn rollback_sqlite(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("INSERT INTO delivery_ledger (guild_id, run_id, active_delivery_id, occurred_at) VALUES (?, ?, ?, ?)")
        .bind(42_i64).bind("rollback").bind("must-not-commit").bind("2026-09-21T12:10:00Z").execute(&mut *tx).await?;
    tx.rollback().await
}

pub async fn rollback_postgres(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO delivery_ledger (guild_id, run_id, active_delivery_id, occurred_at) VALUES ($1, $2, $3, $4)")
        .bind(42_i64).bind("rollback").bind("must-not-commit").bind("2026-09-21T12:10:00Z").execute(&mut *tx).await?;
    tx.rollback().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixture;

    #[tokio::test]
    async fn sqlite_write_rollback_snapshot_and_vacuum_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.sqlite");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = setup_sqlite(&url).await.unwrap();
        write_sqlite(&pool, &fixture()).await.unwrap();
        rollback_sqlite(&pool).await.unwrap();
        assert_eq!(snapshot_sqlite(&pool).await.unwrap(), fixture());
        let backup = directory.path().join("store.vacuum");
        sqlx::query(&format!(
            "VACUUM INTO '{}'",
            backup.display().to_string().replace('\'', "''")
        ))
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
        let restored = setup_sqlite(&format!("sqlite://{}?mode=rw", backup.display()))
            .await
            .unwrap();
        assert_eq!(snapshot_sqlite(&restored).await.unwrap(), fixture());
    }

    #[tokio::test]
    async fn postgres_write_rollback_and_snapshot() {
        let Some(url) = std::env::var("KANADE_STACK_PG_URL").ok() else {
            eprintln!("skipped: KANADE_STACK_PG_URL is not set");
            return;
        };
        let pool = setup_postgres(&url).await.unwrap();
        sqlx::query("TRUNCATE delivery_ledger")
            .execute(&pool)
            .await
            .unwrap();
        write_postgres(&pool, &fixture()).await.unwrap();
        rollback_postgres(&pool).await.unwrap();
        assert_eq!(snapshot_postgres(&pool).await.unwrap(), fixture());
    }
}
