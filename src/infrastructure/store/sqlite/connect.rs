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

/// Read-only connections; WAL is persistent, so they inherit it.
pub(super) async fn readers(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .max_connections(READERS)
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
}
