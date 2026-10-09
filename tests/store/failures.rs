//! Failure paths: unreadable rows, transactions SQLite aborts on its own, and
//! commits cancelled mid-flight.

use std::time::Duration;

use kanade::domain::schedule::{Change, ChangeSet};
use kanade::domain::scheduler::{ScheduleStore, Scope, StoreError};
use kanade::infrastructure::store::SqliteStore;

use crate::support::{TempDir, commit, fixed, seed, tamper};

#[tokio::test]
async fn unreadable_rows_are_a_typed_backend_error() {
    let dir = TempDir::new();
    let config = dir.config("corrupt");
    let cases = [
        ("weekday", "'[]', 9, '21:30', '[]'"),
        ("time", "'[]', 0, 'half past', '[]'"),
        ("JSON", "'not json', 0, '21:30', '[]'"),
    ];
    for (what, values) in cases {
        SqliteStore::open(&config)
            .await
            .expect("opens")
            .close()
            .await
            .expect("close");
        tamper(
            &config,
            &format!(
                "PRAGMA ignore_check_constraints = ON;
                 DELETE FROM fixed_runs;
                 INSERT INTO fixed_runs VALUES ('bad', '42', NULL, {values}, NULL, 'opt_in', 0);"
            ),
        )
        .await;
        let store = SqliteStore::open(&config).await.expect("reopens");
        let result = store.load(&Scope::All).await;
        assert!(
            matches!(&result, Err(StoreError::Backend(detail)) if detail.contains("stored row is unreadable")),
            "{what}: {result:?}"
        );
        store.close().await.expect("close");
    }
}

#[tokio::test]
async fn a_transaction_sqlite_rolled_back_keeps_its_error_and_the_writer_recovers() {
    let dir = TempDir::new();
    let config = dir.config("abort");
    let store = SqliteStore::open(&config).await.expect("opens");
    seed(&store).await;
    store.close().await.expect("close");
    // RAISE(ROLLBACK) ends the transaction inside SQLite, so the store's own
    // ROLLBACK then fails.
    tamper(
        &config,
        "CREATE TRIGGER abort_boom BEFORE INSERT ON fixed_runs WHEN NEW.id = 'boom'
         BEGIN SELECT RAISE(ROLLBACK, 'injected abort'); END;",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let before = store.load(&Scope::All).await.expect("load");
    for _ in 0..2 {
        let result = store
            .commit(
                before.revision,
                ChangeSet {
                    changes: vec![Change::PutFixedRun(fixed("boom"))],
                },
                kanade::infrastructure::store::conformance::meta(),
            )
            .await;
        assert!(
            matches!(&result, Err(error) if error.to_string().contains("injected abort")),
            "the original error is returned: {result:?}"
        );
        assert_eq!(store.load(&Scope::All).await.expect("load"), before);
    }
    commit(&store, vec![Change::PutFixedRun(fixed("after"))]).await;
    let after = store.load(&Scope::All).await.expect("load");
    assert_eq!(after.revision, before.revision + 1);
    assert!(after.fixed_runs.iter().any(|row| row.id == "after"));
    store.close().await.expect("close");
}

#[tokio::test]
async fn cancelled_commits_land_whole_or_not_at_all() {
    let dir = TempDir::new();
    let store = SqliteStore::open(&dir.config("cancel"))
        .await
        .expect("opens");
    let mut cancelled = 0;
    for (round, micros) in [
        0, 50, 100, 200, 400, 800, 1_600, 3_200, 6_400, 12_800, 25_600, 51_200, 204_800,
    ]
    .into_iter()
    .enumerate()
    {
        let before = store.load(&Scope::All).await.expect("load");
        let changes: Vec<Change> = (0..300)
            .map(|n| Change::PutFixedRun(fixed(&format!("c{round}-{n}"))))
            .collect();
        let attempt = tokio::time::timeout(
            Duration::from_micros(micros),
            store.commit(
                before.revision,
                ChangeSet { changes },
                kanade::infrastructure::store::conformance::meta(),
            ),
        )
        .await;
        match attempt {
            Ok(result) => {
                result.expect("an uncancelled commit succeeds");
            }
            Err(_) => cancelled += 1,
        }
        // A cancelled COMMIT may still be finishing on the writer, so the
        // follow-up re-reads the revision on conflict; after it, all is settled.
        let follow_up = ChangeSet {
            changes: vec![Change::PutFixedRun(fixed(&format!("next-{round}")))],
        };
        loop {
            let revision = store.load(&Scope::All).await.expect("load").revision;
            match store
                .commit(
                    revision,
                    follow_up.clone(),
                    kanade::infrastructure::store::conformance::meta(),
                )
                .await
            {
                Err(StoreError::Conflict { .. }) => continue,
                result => {
                    result.expect("the writer still commits");
                    break;
                }
            }
        }
        let state = store.load(&Scope::All).await.expect("load");
        let landed = state
            .fixed_runs
            .iter()
            .filter(|row| row.id.starts_with(&format!("c{round}-")))
            .count();
        assert!(landed == 0 || landed == 300, "round {round}: {landed} rows");
        let own = u64::from(landed == 300) + 1;
        assert_eq!(state.revision, before.revision + own, "round {round}");
    }
    assert!(cancelled > 0, "at least one commit was cancelled");
    store.close().await.expect("close");
}

#[tokio::test]
async fn a_busy_begin_is_returned_and_the_next_commit_reconnects() {
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{ConnectOptions, Connection};

    let dir = TempDir::new();
    let config = dir.config("busy-begin");
    let store = SqliteStore::open(&config).await.expect("opens");
    seed(&store).await;
    let before = store.load(&Scope::All).await.expect("load");
    let mut holder = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .connect()
        .await
        .expect("raw connection");
    sqlx::raw_sql("BEGIN IMMEDIATE")
        .execute(&mut holder)
        .await
        .expect("external write lock");
    let result = store
        .commit(
            before.revision,
            ChangeSet {
                changes: vec![Change::PutFixedRun(fixed("blocked"))],
            },
            kanade::infrastructure::store::conformance::meta(),
        )
        .await;
    assert!(
        matches!(&result, Err(StoreError::Backend(detail)) if detail.contains("locked")),
        "{result:?}"
    );
    sqlx::raw_sql("ROLLBACK")
        .execute(&mut holder)
        .await
        .expect("release");
    holder.close().await.expect("close");
    commit(&store, vec![Change::PutFixedRun(fixed("after-busy"))]).await;
    assert_eq!(
        store.load(&Scope::All).await.expect("load").revision,
        before.revision + 1
    );
    store
        .close()
        .await
        .expect("close waits for the orphaned connection");
}
