//! Connection settings applied to every connection the store opens.

use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, SqliteConnection, SqlitePool};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const READERS: u32 = 4;

// Foreign keys are connection-local in SQLite, so every connection sets them.
fn options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .foreign_keys(true)
        .busy_timeout(BUSY_TIMEOUT)
        .synchronous(SqliteSynchronous::Full)
}

/// The single write connection; creates the file and switches it to WAL.
pub(super) async fn writer(path: &Path) -> Result<SqliteConnection, sqlx::Error> {
    options(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .connect()
        .await
}

/// Read-only connections; WAL is persistent, so they inherit it. Waiting
/// for a free one is bounded like a lock wait (SQLx's default is 30 s, as
/// long as Compose's whole stop grace); a timed-out read fails as a backend
/// error.
pub(super) async fn readers(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .max_connections(READERS)
        .acquire_timeout(BUSY_TIMEOUT)
        .connect_with(options(path).read_only(true))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No authorizer is installed (sqlx exposes none without `unsafe` FFI),
    /// so this pins what SQLite itself already denies: extension loading
    /// stays disabled on every store connection.
    #[tokio::test]
    async fn extension_loading_is_disabled() {
        let dir = std::env::temp_dir().join(format!("kanade-connect-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).expect("temp dir");
        let path = dir.join("db.sqlite3");
        let mut conn = writer(&path).await.expect("writer");
        let result = sqlx::query("SELECT load_extension('kanade-missing-extension')")
            .execute(&mut conn)
            .await;
        let _ = sqlx::Connection::close(conn).await;
        let _ = std::fs::remove_dir_all(&dir);
        let error = result.expect_err("refused").to_string();
        assert!(error.contains("not authorized"), "{error}");
    }

    /// Every reader held (e.g. by slow admin reads): a read waits at most
    /// the acquire timeout, then fails as a database error; once reads are
    /// refused, a waiting read and every later one fail at once.
    #[tokio::test]
    async fn reads_wait_for_a_reader_only_so_long_and_refused_reads_end_at_once() {
        use std::os::unix::fs::DirBuilderExt;

        use super::super::{SqliteStore, SqliteStoreConfig, SqliteStoreError};

        let root = std::fs::canonicalize(std::env::temp_dir()).expect("temp dir");
        let dir = root.join(format!("kanade-readers-{}", uuid::Uuid::new_v4()));
        for path in [dir.clone(), dir.join("locks")] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .expect("create");
        }
        let store = SqliteStore::open(&SqliteStoreConfig {
            db_path: dir.join("db.sqlite3"),
            owner_lock_dir: dir.join("locks"),
        })
        .await
        .expect("open");
        let pool = store.reader_pool();
        let mut held = Vec::new();
        for _ in 0..READERS {
            held.push(pool.acquire().await.expect("a reader"));
        }

        let started = tokio::time::Instant::now();
        let timed_out = store.schema_version().await;
        let waited = started.elapsed();
        assert!(
            matches!(
                timed_out,
                Err(SqliteStoreError::Database(sqlx::Error::PoolTimedOut))
            ),
            "{timed_out:?}"
        );
        assert!(
            waited >= BUSY_TIMEOUT - Duration::from_millis(50)
                && waited < BUSY_TIMEOUT + Duration::from_secs(2),
            "{waited:?}"
        );

        let (waiting, refused) = tokio::join!(store.schema_version(), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let at = tokio::time::Instant::now();
            store.refuse_reads();
            at
        });
        assert!(
            matches!(
                waiting,
                Err(SqliteStoreError::Database(sqlx::Error::PoolClosed))
            ),
            "{waiting:?}"
        );
        assert!(refused.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            store.schema_version().await,
            Err(SqliteStoreError::Database(sqlx::Error::PoolClosed))
        ));

        drop(held);
        store.close().await.expect("close");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
