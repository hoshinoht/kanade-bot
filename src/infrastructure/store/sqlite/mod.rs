//! The SQLite store: one owned write connection plus a small read pool over a
//! WAL database, with embedded checksummed migrations.
//!
//! Exactly one [`SqliteStore`] may own a database (see [`SqliteStore::open`]).
//! `kanade ctl` and every other writer stay HTTP clients of the owning process;
//! nothing else opens the file for writing.

#[macro_use]
mod txn;

mod auth_audit;
mod backup;
mod characters;
mod chardata;
mod connect;
mod debug_cards;
mod decline_notices;
mod drafts;
#[cfg(any(test, feature = "test-support"))]
mod faults;
mod history;
#[cfg(any(test, feature = "test-support"))]
mod hold;
mod journal;
mod maintenance;
mod members;
mod migrate;
mod model_log;
mod owner;
mod owner_requests;
mod picks;
mod proposal_cards;
mod proposals;
mod reminder_cards;
mod replays;
mod rows;
mod run_prompts;
mod schedule;
mod settings;
mod snapshots;
#[cfg(any(test, feature = "test-support"))]
mod temp;
mod web_sessions;
pub(super) mod writer;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sqlx::{Connection, SqlitePool};

pub use backup::{BACKUP_MANIFEST_FORMAT, BackupManifest, Seal, SwapOwnership};
pub use chardata::SqliteCharData;
use owner::StoreOwner;
#[cfg(any(test, feature = "test-support"))]
pub use snapshots::ReadPause;
#[cfg(any(test, feature = "test-support"))]
pub use temp::TempStoreDir;

use crate::domain::history::{ChangeHistory, ChangeRef};
use writer::Writer;

/// How long [`SqliteStore::close`] waits for readers handed back late.
const READER_DRAIN: std::time::Duration = std::time::Duration::from_secs(5);

/// A pooled read connection from [`SqliteStore::reader`].
pub(super) struct Reader {
    conn: sqlx::pool::PoolConnection<sqlx::Sqlite>,
    /// After `conn`: the clock goes only once the connection is handed back.
    #[cfg(any(test, feature = "test-support"))]
    _hold: hold::ClockHold,
}

impl Reader {
    /// Close the connection instead of returning it to the pool.
    pub(super) fn close_on_drop(&mut self) {
        self.conn.close_on_drop();
    }
}

impl std::ops::Deref for Reader {
    type Target = sqlx::SqliteConnection;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl std::ops::DerefMut for Reader {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

/// Where the database lives and where its owner lock is taken.
///
/// Both must be absolute paths without `..` or symbolic links. The database's
/// directory must be private to this user (0700 or stricter) and the file
/// 0600; it is created if absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqliteStoreConfig {
    pub db_path: PathBuf,
    /// The operator-configured private owner lock directory (maintenance
    /// contract `DB_OWNER_LOCK_DIR`): an existing directory owned by this user
    /// with mode exactly 0700, shared by every writer entrypoint. Lockfiles
    /// in it are named by the database's device and inode, never its path.
    pub owner_lock_dir: PathBuf,
}

/// Opening, migrating, owning or copying a store failed.
#[derive(Debug, thiserror::Error)]
pub enum SqliteStoreError {
    /// Another owner, in this process or another, holds the database inode.
    #[error("database {} is already owned by another writer", .db_path.display())]
    Owned { db_path: PathBuf },
    /// A configured path is not safe to own a database through.
    #[error("refusing unsafe store path {}: {reason}", .path.display())]
    UnsafePath { path: PathBuf, reason: &'static str },
    /// The configured path no longer names the owned database file, so
    /// writes were refused.
    #[error(
        "database {} was moved or replaced after opening; writes are refused",
        .db_path.display()
    )]
    IdentityChanged { db_path: PathBuf },
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("sqlite: {0}")]
    Database(#[from] sqlx::Error),
    /// An applied migration no longer matches the embedded one.
    #[error("migration {version} was applied with different content")]
    ChecksumMismatch { version: i64 },
    /// The database was migrated by a newer build.
    #[error("database schema version {found} is newer than this build ({known})")]
    FutureVersion { found: i64, known: i64 },
    /// Applied versions are not a contiguous prefix of the known migrations.
    #[error("applied migrations skip or reorder version {version}")]
    MigrationGap { version: i64 },
    /// A backup or restore would overwrite an existing file.
    #[error("refusing to overwrite {}", .path.display())]
    Exists { path: PathBuf },
    /// The history no longer contains an externally kept anchor record: it
    /// was truncated or rewritten behind it.
    #[error(
        "change history no longer contains anchored change {} ({})",
        .anchor.seq,
        .anchor.hash
    )]
    AnchorMissing { anchor: ChangeRef },
    /// A backup failed validation before restore; nothing was published.
    #[error("backup {} is unusable: {detail}", .path.display())]
    InvalidBackup { path: PathBuf, detail: String },
    /// SQLite takes backup paths as UTF-8 text.
    #[error("path is not UTF-8: {}", .path.display())]
    NonUtf8Path { path: PathBuf },
}

impl SqliteStoreError {
    fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Self + '_ {
        move |source| Self::Io {
            path: path.to_owned(),
            source,
        }
    }
}

/// The owning handle on one SQLite database.
///
/// Call [`SqliteStore::close`] to shut down: it closes every SQLite
/// connection before releasing ownership, as the maintenance contract
/// requires. Dropping the store instead releases the lock while connections
/// may still be closing on their worker threads, so a drop without `close`
/// logs `store_dropped_unclosed`.
pub struct SqliteStore {
    writer: Writer,
    readers: SqlitePool,
    /// Told the runs each committed change touched.
    runs_written: crate::infrastructure::store::observer::Observer,
    /// Told what kind of data each committed write changed.
    written: crate::infrastructure::store::observer::WriteHook,
    unclosed: UnclosedWarning,
    /// Test support: a pause the next composed read takes.
    #[cfg(any(test, feature = "test-support"))]
    read_pause: snapshots::PauseSlot,
    /// Test support: armed failures (`faults.rs`).
    #[cfg(any(test, feature = "test-support"))]
    faults: faults::Faults,
    // After the SQLite handles: they drop before ownership is released.
    owner: StoreOwner,
    /// Test support: the directory of an `open_temp` store. Declared after
    /// `owner`, so it is removed only once ownership is released.
    #[cfg(any(test, feature = "test-support"))]
    temp_dir: Option<TempStoreDir>,
}

struct UnclosedWarning {
    armed: bool,
    db_path: PathBuf,
}

impl Drop for UnclosedWarning {
    fn drop(&mut self) {
        if self.armed {
            crate::runtime::logging::store_dropped_unclosed(&self.db_path);
        }
    }
}

impl SqliteStore {
    /// Install the run-write hook (once; `false` if already set). It runs
    /// after each committed `commit`/`commit_merge` with the touched runs.
    pub fn observe_run_writes(&self, observer: crate::infrastructure::store::RunObserver) -> bool {
        self.runs_written.set(observer)
    }

    pub(super) fn runs_written(&self, runs: &[String]) {
        self.runs_written.notify(runs);
    }

    /// Install the write-kind hook (once; `false` if already set). It runs
    /// after each committed write that live pages read.
    pub fn observe_writes(&self, observer: crate::infrastructure::store::WriteObserver) -> bool {
        self.written.set(observer)
    }

    pub(super) fn written(&self) -> &crate::infrastructure::store::observer::WriteHook {
        &self.written
    }

    pub(super) async fn writer_lease(&self) -> Result<writer::WriterLease<'_>, writer::LeaseError> {
        self.writer.lease().await
    }

    /// A read connection. Every read goes through here, never the pool,
    /// so test support can hold a paused clock still while it waits on one.
    pub(super) async fn reader(&self) -> Result<Reader, sqlx::Error> {
        #[cfg(any(test, feature = "test-support"))]
        let hold = hold::ClockHold::new();
        Ok(Reader {
            conn: self.readers.acquire().await?,
            #[cfg(any(test, feature = "test-support"))]
            _hold: hold,
        })
    }

    /// From now on every read fails at once (`PoolClosed`, a backend error),
    /// waiting ones included; reads in progress and every write are
    /// unaffected. For serve's shutdown cutoff, so later reads cannot each
    /// wait out the acquire timeout. [`Self::close`] still closes the pool.
    pub fn refuse_reads(&self) {
        // Marks the pool closed on the call; the returned future would only
        // wait for connections in use to come back.
        drop(self.readers.close());
    }

    /// The reader pool itself, for tests that hold every reader.
    #[cfg(test)]
    pub(crate) fn reader_pool(&self) -> sqlx::SqlitePool {
        self.readers.clone()
    }

    /// Take ownership (opening or creating the database file), then create
    /// or migrate the schema.
    ///
    /// Opening runs as its own task: a cancelled caller never releases
    /// ownership while SQLite connections are still open, since the task
    /// finishes and then closes the store before the lock is dropped.
    ///
    /// # Errors
    /// [`SqliteStoreError::UnsafePath`] for an unsafe directory or file,
    /// [`SqliteStoreError::Owned`] while another owner holds the database, or
    /// any migration refusal.
    pub async fn open(config: &SqliteStoreConfig) -> Result<Self, SqliteStoreError> {
        let config = config.clone();
        let abandoned = Arc::new(AtomicBool::new(false));
        let _abandon_on_drop = backup::AbandonOnDrop(Arc::clone(&abandoned));
        let task = tokio::spawn(async move {
            let owner = StoreOwner::acquire(&config)?;
            let store = Self::open_owned(owner, &config.db_path)
                .await
                .map_err(|(error, _owner)| error)?;
            if abandoned.load(Ordering::SeqCst) {
                let _ = store.close().await;
                return Err(SqliteStoreError::Io {
                    path: config.db_path.clone(),
                    source: std::io::Error::other("open abandoned by its caller"),
                });
            }
            Ok(store)
        });
        task.await.map_err(|error| SqliteStoreError::Io {
            path: PathBuf::from("open task"),
            source: std::io::Error::other(error),
        })?
    }

    /// [`Self::open`], then check the history still contains `anchor` (the
    /// history head from the latest backup's manifest or a logged anchor).
    ///
    /// # Errors
    /// As [`Self::open`], or [`SqliteStoreError::AnchorMissing`] (the store
    /// is closed again).
    pub async fn open_with_anchor(
        config: &SqliteStoreConfig,
        anchor: &ChangeRef,
    ) -> Result<Self, SqliteStoreError> {
        let store = Self::open(config).await?;
        match store.contains_anchor(anchor).await {
            Ok(true) => Ok(store),
            Ok(false) => {
                let _ = store.close().await;
                Err(SqliteStoreError::AnchorMissing {
                    anchor: anchor.clone(),
                })
            }
            Err(error) => {
                let _ = store.close().await;
                Err(SqliteStoreError::Io {
                    path: config.db_path.clone(),
                    source: std::io::Error::other(error.to_string()),
                })
            }
        }
    }

    /// Connect and migrate; on failure every connection is closed and the
    /// still-held ownership is handed back.
    async fn open_owned(
        owner: StoreOwner,
        db_path: &Path,
    ) -> Result<Self, (SqliteStoreError, StoreOwner)> {
        #[cfg(any(test, feature = "test-support"))]
        let _hold = hold::ClockHold::new();
        if let Err(error) = owner.identity().verify() {
            return Err((error, owner));
        }
        let mut writer = match connect::writer(db_path).await {
            Ok(writer) => writer,
            Err(error) => return Err((error.into(), owner)),
        };
        if let Err(error) = migrate::apply(&mut writer).await {
            let _ = writer.close().await;
            return Err((error, owner));
        }
        if let Err(error) = history::ensure_genesis(&mut writer).await {
            let _ = writer.close().await;
            return Err((error.into(), owner));
        }
        let readers = match connect::readers(db_path).await {
            Ok(pool) => pool,
            Err(error) => {
                let _ = writer.close().await;
                return Err((error.into(), owner));
            }
        };
        Ok(Self {
            writer: Writer::new(db_path.to_owned(), owner.identity(), writer),
            readers,
            runs_written: Default::default(),
            written: Default::default(),
            #[cfg(any(test, feature = "test-support"))]
            read_pause: Default::default(),
            #[cfg(any(test, feature = "test-support"))]
            faults: Default::default(),
            unclosed: UnclosedWarning {
                armed: true,
                db_path: db_path.to_owned(),
            },
            owner,
            #[cfg(any(test, feature = "test-support"))]
            temp_dir: None,
        })
    }

    /// Close every connection, including any a cancelled or failed
    /// transaction orphaned, then release ownership.
    ///
    /// # Errors
    /// Closing the write connection failed; ownership is released regardless.
    pub async fn close(self) -> Result<(), SqliteStoreError> {
        #[cfg(any(test, feature = "test-support"))]
        let (store, temp_dir) = {
            let mut store = self;
            let temp_dir = store.temp_dir.take();
            (store, temp_dir)
        };
        #[cfg(not(any(test, feature = "test-support")))]
        let store = self;
        let (closed, owner) = store.close_connections().await;
        drop(owner);
        // The lock file lives in it: removed only after ownership is released.
        #[cfg(any(test, feature = "test-support"))]
        drop(temp_dir);
        closed
    }

    /// [`Self::close`] without releasing ownership: every connection is
    /// closed and the still-held ownership is handed back.
    async fn close_connections(self) -> (Result<(), SqliteStoreError>, StoreOwner) {
        let Self {
            writer,
            readers,
            mut unclosed,
            owner,
            ..
        } = self;
        #[cfg(any(test, feature = "test-support"))]
        let _hold = hold::ClockHold::new();
        unclosed.armed = false;
        // A reader handed back while the pool closes (SQLx 0.8 pings it
        // first, after its closed check) lands in the idle queue after
        // `close` has emptied it, and `close` still returns once every
        // permit is back. Dropped there, it would close later on its own
        // thread, after the writer: the writer's close then cannot take the
        // lock to checkpoint, and the read-only straggler cannot either, so
        // the `-wal` stays and SQLite runs past the ownership release. Each
        // further `close` closes what is idle, so go on until none is left,
        // for at most `READER_DRAIN`: shutdown must not hang on a reader
        // that never comes back.
        readers.close().await;
        let drained = tokio::time::timeout(READER_DRAIN, async {
            while readers.size() > 0 {
                // A connection SQLx itself has out (its idle reaper) is let
                // go by its own task: give that task its turn.
                tokio::task::yield_now().await;
                readers.close().await;
            }
        })
        .await;
        if drained.is_err() {
            crate::runtime::logging::log!(warn, "store_readers_not_drained", open = readers.size());
        }
        let closed = writer.shutdown().await;
        (closed.map_err(Into::into), owner)
    }

    /// The highest applied migration.
    ///
    /// # Errors
    /// The read failed.
    pub async fn schema_version(&self) -> Result<i64, SqliteStoreError> {
        let version = sqlx::query_scalar("SELECT schema_version FROM store_meta WHERE id = 1")
            .fetch_one(&mut *self.reader().await?)
            .await?;
        Ok(version)
    }

    /// Rows `PRAGMA foreign_key_check` reports; zero on a sound store.
    ///
    /// # Errors
    /// The check failed to run.
    pub async fn foreign_key_violations(&self) -> Result<usize, SqliteStoreError> {
        let rows = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut *self.reader().await?)
            .await?;
        Ok(rows.len())
    }
}
