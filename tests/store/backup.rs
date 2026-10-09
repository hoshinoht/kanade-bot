//! `VACUUM INTO` backups and restore-by-copy.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kanade::domain::model_log::ModelLogStore;
use kanade::domain::notify::DeclineNoticeStore;
use kanade::domain::schedule::{Change, ScheduleSnapshot};
use kanade::domain::scheduler::{ScheduleStore, Scope};
use kanade::infrastructure::store::{SqliteStore, SqliteStoreError};

use crate::support::{TempDir, commit, fixed, partials, seed, tamper};

#[tokio::test]
async fn backup_restores_to_an_equal_store_and_never_overwrites() {
    let dir = TempDir::new();
    let store = SqliteStore::open(&dir.config("live")).await.expect("opens");
    seed(&store).await;
    let live = store.load(&Scope::All).await.expect("load");
    let copy = dir.path().join("copy.sqlite3");
    store.backup(&copy).await.expect("backup");
    let again = store.backup(&copy).await.expect_err("refused");
    assert!(matches!(again, SqliteStoreError::Exists { .. }), "{again}");

    let restored = SqliteStore::restore(&copy, &dir.config("restored"))
        .await
        .expect("restores");
    assert_eq!(restored.load(&Scope::All).await.expect("load"), live);
    assert_eq!(restored.schema_version().await.expect("version"), 33);
    restored.close().await.expect("close");

    let occupied = SqliteStore::restore(&copy, &dir.config("live"))
        .await
        .err()
        .expect("refused");
    assert!(
        matches!(occupied, SqliteStoreError::Exists { .. }),
        "the live database is never overwritten: {occupied}"
    );
    store.close().await.expect("close");
    assert!(partials(dir.path()).is_empty(), "staging is removed");
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

#[tokio::test]
async fn backups_and_restored_files_are_private() {
    let dir = TempDir::new();
    let store = SqliteStore::open(&dir.config("private"))
        .await
        .expect("opens");
    seed(&store).await;
    let copy = dir.path().join("private-copy.sqlite3");
    store.backup(&copy).await.expect("backup");
    assert_eq!(mode(&copy), 0o600);
    let config = dir.config("private-restored");
    let restored = SqliteStore::restore(&copy, &config)
        .await
        .expect("restores");
    assert_eq!(mode(&config.db_path), 0o600);
    restored.close().await.expect("close");
    store.close().await.expect("close");
}

#[tokio::test]
async fn a_cancelled_backup_leaves_nothing_behind() {
    let dir = TempDir::new();
    let store = SqliteStore::open(&dir.config("cancel"))
        .await
        .expect("opens");
    seed(&store).await;
    let copy = dir.path().join("cancelled.sqlite3");
    let attempt = tokio::time::timeout(Duration::ZERO, store.backup(&copy)).await;
    assert!(attempt.is_err(), "the backup was cancelled mid-copy");
    // The copy finishes in the background, then its staging is removed.
    for _ in 0..500 {
        if partials(dir.path()).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(partials(dir.path()).is_empty(), "staging is removed");
    assert!(!copy.exists(), "an abandoned backup is not published");
    store.close().await.expect("close");
}

/// Restore `backup`, expecting a refusal that publishes nothing.
async fn refused_restore(dir: &TempDir, backup: &Path, name: &str) -> SqliteStoreError {
    let config = dir.config(name);
    let error = SqliteStore::restore(backup, &config)
        .await
        .err()
        .expect("refused");
    assert!(!config.db_path.exists(), "{name}: nothing was published");
    assert!(
        partials(dir.path()).is_empty(),
        "{name}: staging is removed"
    );
    error
}

#[tokio::test]
async fn restore_refuses_stray_journal_files_and_keeps_them() {
    let dir = TempDir::new();
    let store = SqliteStore::open(&dir.config("source"))
        .await
        .expect("opens");
    seed(&store).await;
    let copy = dir.path().join("source-copy.sqlite3");
    store.backup(&copy).await.expect("backup");
    store.close().await.expect("close");
    for suffix in ["-wal", "-shm", "-journal"] {
        let name = format!("stray{suffix}");
        let stray = PathBuf::from(format!("{}{suffix}", dir.config(&name).db_path.display()));
        std::fs::write(&stray, b"leftover").expect("plant");
        let error = refused_restore(&dir, &copy, &name).await;
        assert!(
            matches!(&error, SqliteStoreError::Exists { path } if *path == stray),
            "{error}"
        );
        assert_eq!(std::fs::read(&stray).expect("kept"), b"leftover");
    }
}

#[tokio::test]
async fn restore_validates_the_copy_before_publishing() {
    let dir = TempDir::new();
    let garbage = dir.path().join("garbage.sqlite3");
    std::fs::write(&garbage, vec![0x5a_u8; 8192]).expect("garbage");
    let error = refused_restore(&dir, &garbage, "from-garbage").await;
    assert!(
        matches!(error, SqliteStoreError::InvalidBackup { .. }),
        "{error}"
    );

    let future = dir.config("future");
    SqliteStore::open(&future)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(
        &future,
        "INSERT INTO schema_migrations VALUES (34, 'next', '2027-01-01T00:00:00+00:00')",
    )
    .await;
    let error = refused_restore(&dir, &future.db_path, "from-future").await;
    assert!(
        matches!(
            error,
            SqliteStoreError::FutureVersion {
                found: 34,
                known: 33
            }
        ),
        "{error}"
    );
}

/// A backup taken before schema v20 (here version 18, also before 0019)
/// restores and migrates; its logs read as "not reported".
#[tokio::test]
async fn a_pre_v20_backup_restores_and_migrates() {
    let dir = TempDir::new();
    let old = dir.config("pre-v19");
    SqliteStore::open(&old)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(
        &old,
        "INSERT INTO extractions (id, at, member_ids, model, prompt, raw_response, \
         request_count, outcome, guardrail, message_ids, proposal_ids) VALUES ('x-1', \
         '2026-09-01T00:00:00.000000+00:00', '[]', 'm', 'p', 'r', 1, 'no_change', '{}', \
         '[]', '[]');
         INSERT INTO chat_interactions (id, at, question, reply, outcome, clean_retry, \
         withheld, guardrail, request_count) VALUES ('c-1', \
         '2026-09-01T00:00:00.000000+00:00', 'q', 'r', 'answered', 0, 0, '{}', 1);
         INSERT INTO chat_rounds (interaction_id, ord, model, tool_bundles, tools, \
         tool_calls) VALUES ('c-1', 0, 'kanata/chat', '[]', '[]', '[]');
         ALTER TABLE extractions DROP COLUMN prompt_estimate;
         ALTER TABLE extractions DROP COLUMN completion_tokens;
         ALTER TABLE extractions DROP COLUMN prompt_tokens;
         ALTER TABLE chat_rounds DROP COLUMN prompt_estimate;
         ALTER TABLE chat_rounds DROP COLUMN completion_tokens;
         ALTER TABLE chat_rounds DROP COLUMN prompt_tokens;
         DROP INDEX decline_notices_pending;
         ALTER TABLE decline_notices DROP COLUMN retract_pending;
         ALTER TABLE decline_notices DROP COLUMN display_name;
         ALTER TABLE decline_notices DROP COLUMN reference_id;
         INSERT INTO decline_notices (run_id, user_id, channel_id, message_id, notified_at)
         VALUES ('old-run', 'old-member', 'old-channel', 'old-message',
                 '2026-09-01T00:00:00.000000+00:00');
         ALTER TABLE extractions DROP COLUMN session_id;
         ALTER TABLE extractions DROP COLUMN request_ids;
         ALTER TABLE chat_interactions DROP COLUMN session_id;
         ALTER TABLE chat_rounds DROP COLUMN request_ids;
         ALTER TABLE web_sessions DROP COLUMN superseded_until;
         ALTER TABLE web_sessions DROP COLUMN client_tag;
         ALTER TABLE web_sessions DROP COLUMN device;
         ALTER TABLE web_sessions DROP COLUMN avatar_hash;
         DROP TABLE settings_changes;
         DROP TABLE rewrites;
         DROP TABLE header_overrides;
         DROP TABLE idempotency_replays;
         DROP TABLE auth_audit;
         ALTER TABLE fixed_runs DROP COLUMN owner_pinned;
         DELETE FROM schema_migrations WHERE version >= 19;
         ALTER TABLE extractions DROP COLUMN reasoning_content;
         ALTER TABLE extractions DROP COLUMN reasoning_tokens;
         ALTER TABLE chat_rounds DROP COLUMN reasoning_content;
         ALTER TABLE chat_rounds DROP COLUMN reasoning_tokens;
         UPDATE store_meta SET schema_version = 18;",
    )
    .await;
    let restored = SqliteStore::restore(&old.db_path, &dir.config("from-pre-v19"))
        .await
        .expect("restores");
    assert_eq!(restored.schema_version().await.expect("version"), 33);
    assert_eq!(restored.foreign_key_violations().await.expect("check"), 0);
    let decline = restored
        .decline_notice("old-run", "old-member")
        .await
        .expect("decline reads")
        .expect("old decline survives");
    assert_eq!(decline.reference_id, None);
    assert_eq!(decline.display_name, None);
    assert!(!decline.retract_pending);
    let log = restored
        .load_extraction("x-1")
        .await
        .expect("load")
        .expect("kept");
    assert_eq!(
        (
            log.prompt_tokens,
            log.completion_tokens,
            log.prompt_estimate
        ),
        (None, None, None)
    );
    let chat = restored
        .load_chat("c-1")
        .await
        .expect("load")
        .expect("kept");
    let round = &chat.rounds[0];
    assert_eq!(
        (
            round.prompt_tokens,
            round.completion_tokens,
            round.prompt_estimate
        ),
        (None, None, None)
    );
    restored.close().await.expect("close");
}

const COMMITS: usize = 200;
const BEFORE_BACKUP: usize = 10;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_while_a_writer_commits_is_a_consistent_revision() {
    let dir = TempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.config("busy")).await.expect("opens"));
    let states: Arc<Mutex<BTreeMap<u64, ScheduleSnapshot>>> = Arc::default();
    let initial = store.load(&Scope::All).await.expect("load");
    states.lock().unwrap().insert(initial.revision, initial);

    let writer = {
        let (store, states) = (Arc::clone(&store), Arc::clone(&states));
        tokio::spawn(async move {
            for n in 0..COMMITS {
                commit(&*store, vec![Change::PutFixedRun(fixed(&format!("f-{n}")))]).await;
                let state = store.load(&Scope::All).await.expect("load");
                states.lock().unwrap().insert(state.revision, state);
            }
        })
    };
    while states.lock().unwrap().len() <= BEFORE_BACKUP {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    let copy = dir.path().join("mid-write.sqlite3");
    store.backup(&copy).await.expect("backup while writing");
    writer.await.expect("writer finishes");

    let restored = SqliteStore::restore(&copy, &dir.config("mid-restored"))
        .await
        .expect("restores");
    let snapshot = restored.load(&Scope::All).await.expect("load");
    assert!(
        snapshot.revision >= BEFORE_BACKUP as u64,
        "{}",
        snapshot.revision
    );
    let expected = states.lock().unwrap().get(&snapshot.revision).cloned();
    assert_eq!(
        Some(snapshot),
        expected,
        "the copy is one committed revision"
    );
    restored.close().await.expect("close");
    Arc::into_inner(store)
        .expect("writer released the store")
        .close()
        .await
        .expect("close");
}

#[tokio::test]
async fn a_cancelled_restore_leaves_nothing_behind() {
    let dir = TempDir::new();
    let store = SqliteStore::open(&dir.config("source"))
        .await
        .expect("opens");
    seed(&store).await;
    let copy = dir.path().join("source-copy.sqlite3");
    store.backup(&copy).await.expect("backup");
    store.close().await.expect("close");

    let config = dir.config("cancelled");
    let attempt = tokio::time::timeout(Duration::ZERO, SqliteStore::restore(&copy, &config)).await;
    assert!(attempt.is_err(), "the restore was cancelled");
    // The task finishes on its own, then removes its staging.
    for _ in 0..500 {
        if partials(dir.path()).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(partials(dir.path()).is_empty(), "staging is removed");
    assert!(
        !config.db_path.exists(),
        "an abandoned restore publishes nothing"
    );
    SqliteStore::restore(&copy, &config)
        .await
        .expect("a later restore succeeds")
        .close()
        .await
        .expect("close");
}
