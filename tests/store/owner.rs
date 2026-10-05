//! Single-writer ownership: private lock directory, inode-keyed lockfiles,
//! the in-process registry, path safety and the retained database identity.

use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;

use kanade::domain::notify::{DeliveryJournal, JournalError};
use kanade::domain::schedule::{Change, ChangeSet};
use kanade::domain::scheduler::{ScheduleStore, Scope, StoreError};
use kanade::infrastructure::store::{SqliteStore, SqliteStoreConfig, SqliteStoreError};

use crate::support::{TempDir, at, fixed, private_dir, seed};

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
}

fn lockfile(config: &SqliteStoreConfig) -> std::path::PathBuf {
    let meta = fs::metadata(&config.db_path).expect("db exists");
    config
        .owner_lock_dir
        .join(format!("kanade-{:x}-{:x}.lock", meta.dev(), meta.ino()))
}

async fn refusal(config: &SqliteStoreConfig) -> SqliteStoreError {
    SqliteStore::open(config).await.err().expect("refused")
}

async fn unsafe_reason(config: &SqliteStoreConfig) -> &'static str {
    match refusal(config).await {
        SqliteStoreError::UnsafePath { reason, .. } => reason,
        other => panic!("expected an unsafe path: {other}"),
    }
}

#[tokio::test]
async fn second_writer_is_refused_until_the_owner_closes() {
    let dir = TempDir::new();
    let config = dir.config("owned");
    let owner = SqliteStore::open(&config).await.expect("first owner");
    let error = refusal(&config).await;
    assert!(matches!(error, SqliteStoreError::Owned { .. }), "{error}");
    owner.close().await.expect("close");
    let next = SqliteStore::open(&config)
        .await
        .expect("ownership released");
    drop(next);
    SqliteStore::open(&config)
        .await
        .expect("dropping the store also releases ownership")
        .close()
        .await
        .expect("close");
    assert!(lockfile(&config).exists(), "the lockfile is never removed");
}

#[tokio::test]
async fn a_hard_link_alias_contends_for_the_same_database() {
    let dir = TempDir::new();
    let config = dir.config("original");
    let owner = SqliteStore::open(&config).await.expect("owner");
    let alias = dir.config("alias");
    fs::hard_link(&config.db_path, &alias.db_path).expect("hard link");
    let error = refusal(&alias).await;
    assert!(
        matches!(error, SqliteStoreError::Owned { .. }),
        "same process, another path: {error}"
    );
    owner.close().await.expect("close");

    // Another process's kernel lock on the inode's lockfile also refuses.
    let held = File::options()
        .read(true)
        .write(true)
        .open(lockfile(&config))
        .expect("lockfile");
    held.try_lock().expect("outside lock");
    let error = refusal(&alias).await;
    assert!(matches!(error, SqliteStoreError::Owned { .. }), "{error}");
    drop(held);
    SqliteStore::open(&alias)
        .await
        .expect("the alias opens once the lock is free")
        .close()
        .await
        .expect("close");
}

#[tokio::test]
async fn symlinked_directories_and_files_are_refused() {
    let dir = TempDir::new();

    let mut config = dir.config("linked-locks");
    let lock_link = dir.path().join("lock-link");
    symlink(&config.owner_lock_dir, &lock_link).expect("symlink");
    config.owner_lock_dir = lock_link;
    assert_eq!(unsafe_reason(&config).await, "is a symbolic link");

    let real = dir.path().join("real-db-dir");
    private_dir(&real);
    let linked_dir = dir.path().join("db-link");
    symlink(&real, &linked_dir).expect("symlink");
    let mut config = dir.config("linked-dir");
    config.db_path = linked_dir.join("db.sqlite3");
    assert_eq!(unsafe_reason(&config).await, "is a symbolic link");
    assert!(!real.join("db.sqlite3").exists(), "nothing was created");

    let config = dir.config("linked-file");
    let target = dir.path().join("target.sqlite3");
    fs::write(&target, b"").expect("target");
    chmod(&target, 0o600);
    symlink(&target, &config.db_path).expect("symlink");
    assert_eq!(unsafe_reason(&config).await, "is a symbolic link");
}

#[tokio::test]
async fn loose_modes_are_refused() {
    let dir = TempDir::new();
    let config = dir.config("modes");

    chmod(&config.owner_lock_dir, 0o755);
    assert_eq!(unsafe_reason(&config).await, "must have mode 0700");
    chmod(&config.owner_lock_dir, 0o700);

    chmod(dir.path(), 0o750);
    assert_eq!(
        unsafe_reason(&config).await,
        "must not be accessible to group or others (0700)"
    );
    chmod(dir.path(), 0o700);

    fs::write(&config.db_path, b"").expect("db");
    chmod(&config.db_path, 0o644);
    assert_eq!(unsafe_reason(&config).await, "must have mode 0600");
    chmod(&config.db_path, 0o600);

    let lock = lockfile(&config);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .expect("lockfile");
    chmod(&lock, 0o644);
    assert_eq!(unsafe_reason(&config).await, "must have mode 0600");
    chmod(&lock, 0o600);

    SqliteStore::open(&config)
        .await
        .expect("opens once every mode is private")
        .close()
        .await
        .expect("close");
}

#[tokio::test]
async fn writes_are_refused_once_the_database_file_is_swapped() {
    let dir = TempDir::new();
    let config = dir.config("swapped");
    let store = SqliteStore::open(&config).await.expect("opens");
    seed(&store).await;
    let lease = store
        .begin_lease("instance", "delivery", at(30, 7))
        .await
        .expect("lease");
    let before = store.load(&Scope::All).await.expect("load");

    let moved = dir.path().join("moved.sqlite3");
    fs::rename(&config.db_path, &moved).expect("move");
    fs::copy(&moved, &config.db_path).expect("impostor");
    let result = store
        .commit(
            before.revision,
            ChangeSet {
                changes: vec![Change::PutFixedRun(fixed("after-swap"))],
            },
            kanade::infrastructure::store::conformance::meta(),
        )
        .await;
    assert!(
        matches!(&result, Err(StoreError::Backend(detail)) if detail.contains("moved or replaced")),
        "{result:?}"
    );
    let journal = store.end_lease(&lease, at(30, 8)).await;
    assert!(
        matches!(&journal, Err(JournalError::Backend(detail)) if detail.contains("moved or replaced")),
        "{journal:?}"
    );

    fs::remove_file(&config.db_path).expect("remove impostor");
    let removed = store
        .commit(
            before.revision,
            ChangeSet {
                changes: vec![Change::PutFixedRun(fixed("after-removal"))],
            },
            kanade::infrastructure::store::conformance::meta(),
        )
        .await;
    assert!(
        matches!(removed, Err(StoreError::Backend(_))),
        "{removed:?}"
    );

    fs::rename(&moved, &config.db_path).expect("move back");
    store
        .commit(
            before.revision,
            ChangeSet {
                changes: vec![Change::PutFixedRun(fixed("restored"))],
            },
            kanade::infrastructure::store::conformance::meta(),
        )
        .await
        .expect("writes resume once the owned file is back");
    store.close().await.expect("close");
}

#[tokio::test]
async fn a_symlink_to_the_owned_file_still_refuses_writes() {
    let dir = TempDir::new();
    let config = dir.config("symlinked-later");
    let store = SqliteStore::open(&config).await.expect("opens");
    let revision = store.load(&Scope::All).await.expect("load").revision;
    let moved = dir.path().join("moved.sqlite3");
    fs::rename(&config.db_path, &moved).expect("move");
    symlink(&moved, &config.db_path).expect("symlink");
    let result = store
        .commit(
            revision,
            ChangeSet {
                changes: vec![Change::PutFixedRun(fixed("via-link"))],
            },
            kanade::infrastructure::store::conformance::meta(),
        )
        .await;
    assert!(
        matches!(&result, Err(StoreError::Backend(detail)) if detail.contains("moved or replaced")),
        "{result:?}"
    );
    fs::remove_file(&config.db_path).expect("unlink symlink");
    fs::rename(&moved, &config.db_path).expect("move back");
    store.close().await.expect("close");
}

#[tokio::test]
async fn a_backup_of_a_swapped_database_is_refused() {
    let dir = TempDir::new();
    let config = dir.config("swapped-backup");
    let store = SqliteStore::open(&config).await.expect("opens");
    seed(&store).await;
    let moved = dir.path().join("moved.sqlite3");
    fs::rename(&config.db_path, &moved).expect("move");
    fs::copy(&moved, &config.db_path).expect("impostor");
    let dest = dir.path().join("snapshot.sqlite3");
    let error = store.backup(&dest).await.expect_err("refused");
    assert!(
        matches!(error, SqliteStoreError::IdentityChanged { .. }),
        "{error}"
    );
    assert!(!dest.exists(), "nothing was published");
    fs::remove_file(&config.db_path).expect("remove impostor");
    fs::rename(&moved, &config.db_path).expect("move back");
    store.close().await.expect("close");
}

#[tokio::test]
async fn a_cancelled_open_releases_ownership_once_sqlite_is_closed() {
    use std::time::Duration;

    let dir = TempDir::new();
    let config = dir.config("cancelled-open");
    let attempt = tokio::time::timeout(Duration::ZERO, SqliteStore::open(&config)).await;
    assert!(attempt.is_err(), "the open was cancelled");
    // The open task finishes, closes the store, then releases the lock.
    let mut reopened = None;
    for _ in 0..500 {
        match SqliteStore::open(&config).await {
            Ok(store) => {
                reopened = Some(store);
                break;
            }
            Err(SqliteStoreError::Owned { .. }) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(other) => panic!("unexpected: {other}"),
        }
    }
    let store = reopened.expect("ownership was released");
    assert_eq!(store.schema_version().await.expect("version"), 22);
    store.close().await.expect("close");
}
