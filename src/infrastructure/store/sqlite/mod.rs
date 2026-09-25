//! The SQLite store: one owned write connection plus a small read pool over a
//! WAL database, with embedded checksummed migrations.
//!
//! Exactly one [`SqliteStore`] may own a database (see [`SqliteStore::open`]).
//! `kanade ctl` and every other writer stay HTTP clients of the owning process;
//! nothing else opens the file for writing.

#[macro_use]
mod txn;

mod backup;
mod connect;
mod drafts;
mod history;
mod journal;
mod members;
mod migrate;
mod model_log;
mod owner;
mod proposal_cards;
mod proposals;
mod rows;
mod schedule;
mod settings;
mod web_sessions;
pub(super) mod writer;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sqlx::{Connection, SqlitePool};

pub use backup::{BACKUP_MANIFEST_FORMAT, BackupManifest};
use owner::StoreOwner;

use crate::domain::history::{ChangeHistory, ChangeRef};
use writer::Writer;

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
#[derive(Debug)]
pub enum SqliteStoreError {
    /// Another owner, in this process or another, holds the database inode.
    Owned {
        db_path: PathBuf,
    },
    /// A configured path is not safe to own a database through.
    UnsafePath {
        path: PathBuf,
        reason: &'static str,
    },
    /// The configured path no longer names the owned database file, so
    /// writes were refused.
    IdentityChanged {
        db_path: PathBuf,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Database(sqlx::Error),
    /// An applied migration no longer matches the embedded one.
    ChecksumMismatch {
        version: i64,
    },
    /// The database was migrated by a newer build.
    FutureVersion {
        found: i64,
        known: i64,
    },
    /// Applied versions are not a contiguous prefix of the known migrations.
    MigrationGap {
        version: i64,
    },
    /// A backup or restore would overwrite an existing file.
    Exists {
        path: PathBuf,
    },
    /// The history no longer contains an externally kept anchor record: it
    /// was truncated or rewritten behind it.
    AnchorMissing {
        anchor: ChangeRef,
    },
    /// A backup failed validation before restore; nothing was published.
    InvalidBackup {
        path: PathBuf,
        detail: String,
    },
    /// SQLite takes backup paths as UTF-8 text.
    NonUtf8Path {
        path: PathBuf,
    },
}

impl SqliteStoreError {
    fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Self + '_ {
        move |source| Self::Io {
            path: path.to_owned(),
            source,
        }
    }
}

impl From<sqlx::Error> for SqliteStoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl fmt::Display for SqliteStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owned { db_path } => write!(
                f,
                "database {} is already owned by another writer",
                db_path.display()
            ),
            Self::UnsafePath { path, reason } => {
                write!(f, "refusing unsafe store path {}: {reason}", path.display())
            }
            Self::IdentityChanged { db_path } => write!(
                f,
                "database {} was moved or replaced after opening; writes are refused",
                db_path.display()
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Database(error) => write!(f, "sqlite: {error}"),
            Self::ChecksumMismatch { version } => {
                write!(f, "migration {version} was applied with different content")
            }
            Self::FutureVersion { found, known } => write!(
                f,
                "database schema version {found} is newer than this build ({known})"
            ),
            Self::MigrationGap { version } => {
                write!(f, "applied migrations skip or reorder version {version}")
            }
            Self::Exists { path } => write!(f, "refusing to overwrite {}", path.display()),
            Self::AnchorMissing { anchor } => write!(
                f,
                "change history no longer contains anchored change {} ({})",
                anchor.seq, anchor.hash
            ),
            Self::InvalidBackup { path, detail } => {
                write!(f, "backup {} is unusable: {detail}", path.display())
            }
            Self::NonUtf8Path { path } => write!(f, "path is not UTF-8: {}", path.display()),
        }
    }
}

impl std::error::Error for SqliteStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Database(error) => Some(error),
            _ => None,
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
    unclosed: UnclosedWarning,
    // Declared last: SQLite handles drop before ownership is released.
    owner: StoreOwner,
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
    pub(super) async fn writer_lease(&self) -> Result<writer::WriterLease<'_>, writer::LeaseError> {
        self.writer.lease().await
    }

    pub(super) async fn reader(
        &self,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>, sqlx::Error> {
        self.readers.acquire().await
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
            unclosed: UnclosedWarning {
                armed: true,
                db_path: db_path.to_owned(),
            },
            owner,
        })
    }

    /// Close every connection, including any a cancelled or failed
    /// transaction orphaned, then release ownership.
    ///
    /// # Errors
    /// Closing the write connection failed; ownership is released regardless.
    pub async fn close(self) -> Result<(), SqliteStoreError> {
        let Self {
            writer,
            readers,
            mut unclosed,
            owner,
            ..
        } = self;
        unclosed.armed = false;
        readers.close().await;
        let closed = writer.shutdown().await;
        drop(owner);
        Ok(closed?)
    }

    /// The highest applied migration.
    ///
    /// # Errors
    /// The read failed.
    pub async fn schema_version(&self) -> Result<i64, SqliteStoreError> {
        let version = sqlx::query_scalar("SELECT schema_version FROM store_meta WHERE id = 1")
            .fetch_one(&self.readers)
            .await?;
        Ok(version)
    }

    /// Rows `PRAGMA foreign_key_check` reports; zero on a sound store.
    ///
    /// # Errors
    /// The check failed to run.
    pub async fn foreign_key_violations(&self) -> Result<usize, SqliteStoreError> {
        let rows = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&self.readers)
            .await?;
        Ok(rows.len())
    }
}
