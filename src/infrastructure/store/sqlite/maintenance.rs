//! The daily WAL checkpoint, on the write connection.

use super::SqliteStore;
use super::schedule::store_error;
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::maintenance::{StoreMaintenance, WalCheckpoint};

impl StoreMaintenance for SqliteStore {
    async fn checkpoint_wal(&self) -> Result<WalCheckpoint, StoreError> {
        // Outside any transaction (a checkpoint cannot run inside one); the
        // lease holds the writer for this statement only. A cancelled call
        // drops the lease, which orphans and closes the connection.
        let mut lease = self
            .writer_lease()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        let row = sqlx::query_as::<_, (i64, i64, i64)>("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(lease.conn())
            .await;
        lease.finish(row.is_ok());
        let (busy, log, checkpointed) = row.map_err(store_error)?;
        Ok(WalCheckpoint {
            busy: busy != 0,
            log,
            checkpointed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model_log::ModelLogStore;
    use crate::infrastructure::store::SqliteStoreConfig;
    use crate::infrastructure::store::model_log_conformance::extraction;

    #[tokio::test]
    async fn checkpoint_truncates_the_wal() {
        use std::os::unix::fs::DirBuilderExt;

        let root = std::fs::canonicalize(std::env::temp_dir()).expect("temp dir");
        let dir = root.join(format!("kanade-checkpoint-{}", uuid::Uuid::new_v4()));
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
        let at = chrono::DateTime::from_timestamp(1_791_676_800, 0).expect("instant");
        for n in 0..20 {
            store
                .record_extraction(extraction(&format!("x-{n}"), at))
                .await
                .unwrap();
        }
        let wal = dir.join("db.sqlite3-wal");
        assert!(
            std::fs::metadata(&wal).unwrap().len() > 0,
            "writes went to the WAL"
        );

        let done = store.checkpoint_wal().await.unwrap();
        assert!(!done.busy, "{done:?}");
        assert_eq!(std::fs::metadata(&wal).unwrap().len(), 0, "{done:?}");
        // The rows survive and later writes still work.
        assert!(store.load_extraction("x-0").await.unwrap().is_some());
        store
            .record_extraction(extraction("x-after", at))
            .await
            .unwrap();
        store.close().await.expect("close");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
