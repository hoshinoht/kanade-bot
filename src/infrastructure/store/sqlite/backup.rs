//! Consistent file backups and restore-by-copy.
//!
//! Both stage the file in a private (0700) sibling directory, make it 0600,
//! sync it and publish it with a hard link, which never replaces an existing
//! file; a partial or unvalidated copy is never visible under the final name.
//! The staging directory is removed on every exit path, including
//! cancellation.

use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, Row, SqliteConnection};

use super::owner::{self, StoreOwner};
use super::{SqliteStore, SqliteStoreConfig, SqliteStoreError, migrate};
use crate::domain::history::ChangeRef;

/// A private sibling directory holding one staged file; removed on drop.
struct Staging {
    dir: PathBuf,
}

impl Staging {
    fn beside(dest: &Path) -> Result<Self, SqliteStoreError> {
        let name = dest
            .file_name()
            .map_or_else(Default::default, |name| name.to_string_lossy().into_owned());
        let dir = dest.with_file_name(format!(".{name}.partial-{}", uuid::Uuid::new_v4()));
        DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(SqliteStoreError::io(&dir))?;
        Ok(Self { dir })
    }

    fn file(&self) -> PathBuf {
        self.dir.join("staged.sqlite3")
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Marks a backup abandoned when its caller stops waiting.
pub(super) struct AbandonOnDrop(pub(super) Arc<AtomicBool>);

impl Drop for AbandonOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn refuse_existing(path: &Path) -> Result<(), SqliteStoreError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(SqliteStoreError::Exists {
            path: path.to_owned(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SqliteStoreError::io(path)(error)),
    }
}

/// SQLite's journal files for `db`. A leftover one beside a new file would be
/// replayed into it as a hot journal.
fn sidecars(db: &Path) -> [PathBuf; 3] {
    ["-wal", "-shm", "-journal"].map(|suffix| {
        let mut path = db.as_os_str().to_owned();
        path.push(suffix);
        PathBuf::from(path)
    })
}

/// Make `staged` 0600, sync it, link it to `dest` and sync the directory.
fn publish(staged: &Path, dest: &Path) -> Result<(), SqliteStoreError> {
    fs::set_permissions(staged, Permissions::from_mode(0o600))
        .map_err(SqliteStoreError::io(staged))?;
    File::open(staged)
        .and_then(|file| file.sync_all())
        .map_err(SqliteStoreError::io(staged))?;
    fs::hard_link(staged, dest).map_err(|error| match error.kind() {
        std::io::ErrorKind::AlreadyExists => SqliteStoreError::Exists {
            path: dest.to_owned(),
        },
        _ => SqliteStoreError::io(dest)(error),
    })?;
    if let Some(dir) = dest.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        File::open(dir)
            .and_then(|dir| dir.sync_all())
            .map_err(SqliteStoreError::io(dir))?;
    }
    Ok(())
}

/// Integrity and migration-ledger checks on a staged copy, read only.
async fn validate(staged: &Path, backup: &Path) -> Result<(), SqliteStoreError> {
    let invalid = |detail: String| SqliteStoreError::InvalidBackup {
        path: backup.to_owned(),
        detail,
    };
    let mut conn: SqliteConnection = SqliteConnectOptions::new()
        .filename(staged)
        .read_only(true)
        .connect()
        .await
        .map_err(|error| invalid(error.to_string()))?;
    let checked = async {
        let report: Vec<String> = sqlx::query("PRAGMA integrity_check")
            .fetch_all(&mut conn)
            .await
            .map_err(|error| invalid(error.to_string()))?
            .iter()
            .map(|row| row.try_get(0))
            .collect::<Result<_, _>>()?;
        if report != ["ok"] {
            return Err(invalid(format!("integrity_check: {}", report.join("; "))));
        }
        let ledger: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
        )
        .fetch_one(&mut conn)
        .await?;
        if ledger == 0 {
            return Err(invalid("no migration ledger".into()));
        }
        migrate::verify(&mut conn).await.map(|_| ())
    }
    .await;
    let _ = conn.close().await;
    checked
}

impl SqliteStore {
    /// Write a consistent snapshot to `dest` (mode 0600) with `VACUUM INTO`
    /// on a reader, so commits continue meanwhile, and its
    /// [`BackupManifest`] (the snapshot's history head, an external anchor)
    /// beside it. The database path must
    /// name the owned inode before and after the copy.
    ///
    /// The copy runs as its own task: if the caller is cancelled, SQLite
    /// still finishes writing into the staging directory, which is then
    /// removed without publishing. A cancellation that lands after that
    /// check may still leave a complete, valid backup at `dest`.
    ///
    /// # Errors
    /// [`SqliteStoreError::Exists`] if `dest` exists, or the copy failed;
    /// `dest` is then left untouched.
    pub async fn backup(&self, dest: &Path) -> Result<(), SqliteStoreError> {
        let identity = self.owner.identity();
        identity.verify()?;
        refuse_existing(dest)?;
        let manifest_path = BackupManifest::path_for(dest);
        refuse_existing(&manifest_path)?;
        let staging = Staging::beside(dest)?;
        let staged = staging.file();
        let target = staged
            .to_str()
            .ok_or_else(|| SqliteStoreError::NonUtf8Path {
                path: staged.clone(),
            })?
            .to_owned();
        let readers = self.readers.clone();
        let dest = dest.to_owned();
        let abandoned = Arc::new(AtomicBool::new(false));
        let _abandon_on_drop = AbandonOnDrop(Arc::clone(&abandoned));
        let task = tokio::spawn(async move {
            sqlx::query("VACUUM INTO ?1")
                .bind(target)
                .execute(&readers)
                .await?;
            // The snapshot came from the owned inode only if the path still
            // names it after the copy.
            identity.verify()?;
            if abandoned.load(Ordering::SeqCst) {
                return Ok(());
            }
            let manifest = read_manifest(&staged).await?;
            let staged_manifest = staging.dir.join("manifest.json");
            fs::write(&staged_manifest, manifest.to_json())
                .map_err(SqliteStoreError::io(&staged_manifest))?;
            publish(&staged, &dest)?;
            let published = publish(&staged_manifest, &manifest_path);
            drop(staging);
            published
        });
        task.await.map_err(|error| SqliteStoreError::Io {
            path: PathBuf::from("backup task"),
            source: std::io::Error::other(error),
        })?
    }

    /// Validate a copy of `backup`, publish it at `config.db_path` and open
    /// it as a fresh store. Nothing is published unless the copy passes
    /// `integrity_check` and the migration ledger.
    ///
    /// The copy is staged beside the database, and ownership is taken on the
    /// staged inode before validation; publication is a hard link, so the
    /// owned inode is the one that appears at `db_path`.
    ///
    /// The work runs as its own task, so a cancelled caller never leaves
    /// staging behind or a half-published database: if the caller is gone
    /// before publication, nothing is published; if it goes after, the
    /// validated database stays at `db_path` and the task closes the store.
    /// Only the process dying mid-restore can leave the hidden staging
    /// directory beside the database.
    ///
    /// # Errors
    /// [`SqliteStoreError::Exists`] if the database path or any of its
    /// `-wal`/`-shm`/`-journal` files exist (they are never removed),
    /// [`SqliteStoreError::UnsafePath`] for unsafe directories,
    /// [`SqliteStoreError::InvalidBackup`] or a migration refusal for a bad
    /// copy, or copy and open failures. A failed copy is reported against the
    /// backup path whether reading it or writing the staged copy failed.
    pub async fn restore(
        backup: &Path,
        config: &SqliteStoreConfig,
    ) -> Result<Self, SqliteStoreError> {
        let abandoned = Arc::new(AtomicBool::new(false));
        let _abandon_on_drop = AbandonOnDrop(Arc::clone(&abandoned));
        let task = tokio::spawn(restore_task(backup.to_owned(), config.clone(), abandoned));
        task.await.map_err(|error| SqliteStoreError::Io {
            path: PathBuf::from("restore task"),
            source: std::io::Error::other(error),
        })?
    }
}

/// Written beside each backup as `<backup>.manifest.json`: the snapshot's
/// history head, kept outside the database as an anchor that a later
/// [`SqliteStore::open_with_anchor`] checks the chain still contains.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupManifest {
    pub history_head: ChangeRef,
    pub revision: u64,
    pub schema_version: i64,
}

pub const BACKUP_MANIFEST_FORMAT: &str = "kanade.backup.v1";

impl BackupManifest {
    pub fn path_for(backup: &Path) -> PathBuf {
        let mut path = backup.as_os_str().to_owned();
        path.push(".manifest.json");
        PathBuf::from(path)
    }

    fn to_json(&self) -> String {
        serde_json::json!({
            "format": BACKUP_MANIFEST_FORMAT,
            "history_head": {"seq": self.history_head.seq, "hash": self.history_head.hash},
            "revision": self.revision,
            "schema_version": self.schema_version,
        })
        .to_string()
    }

    /// Read a manifest written by [`SqliteStore::backup`].
    ///
    /// # Errors
    /// [`SqliteStoreError::Io`] or [`SqliteStoreError::InvalidBackup`].
    pub fn read(path: &Path) -> Result<Self, SqliteStoreError> {
        let text = fs::read_to_string(path).map_err(SqliteStoreError::io(path))?;
        let invalid = |detail: &str| SqliteStoreError::InvalidBackup {
            path: path.to_owned(),
            detail: format!("manifest: {detail}"),
        };
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| invalid("not JSON"))?;
        if value["format"] != BACKUP_MANIFEST_FORMAT {
            return Err(invalid("unknown format"));
        }
        let head = &value["history_head"];
        Ok(Self {
            history_head: ChangeRef {
                seq: head["seq"].as_u64().ok_or_else(|| invalid("seq"))?,
                hash: head["hash"]
                    .as_str()
                    .ok_or_else(|| invalid("hash"))?
                    .to_owned(),
            },
            revision: value["revision"]
                .as_u64()
                .ok_or_else(|| invalid("revision"))?,
            schema_version: value["schema_version"]
                .as_i64()
                .ok_or_else(|| invalid("schema_version"))?,
        })
    }
}

/// The history head, revision and schema version of a staged snapshot.
async fn read_manifest(staged: &Path) -> Result<BackupManifest, SqliteStoreError> {
    let mut conn: SqliteConnection = SqliteConnectOptions::new()
        .filename(staged)
        .read_only(true)
        .connect()
        .await?;
    let read = async {
        let head = sqlx::query("SELECT seq, hash FROM change_log ORDER BY seq DESC LIMIT 1")
            .fetch_one(&mut conn)
            .await?;
        let meta = sqlx::query("SELECT revision, schema_version FROM store_meta WHERE id = 1")
            .fetch_one(&mut conn)
            .await?;
        let seq: i64 = head.try_get("seq")?;
        let revision: i64 = meta.try_get("revision")?;
        Ok::<_, sqlx::Error>(BackupManifest {
            history_head: ChangeRef {
                seq: u64::try_from(seq).unwrap_or_default(),
                hash: head.try_get("hash")?,
            },
            revision: u64::try_from(revision).unwrap_or_default(),
            schema_version: meta.try_get("schema_version")?,
        })
    }
    .await;
    let _ = conn.close().await;
    Ok(read?)
}

fn abandoned_error(path: &Path) -> SqliteStoreError {
    SqliteStoreError::Io {
        path: path.to_owned(),
        source: std::io::Error::other("restore abandoned by its caller"),
    }
}

async fn restore_task(
    backup: PathBuf,
    config: SqliteStoreConfig,
    abandoned: Arc<AtomicBool>,
) -> Result<SqliteStore, SqliteStoreError> {
    let db = &config.db_path;
    owner::check_dirs(&config)?;
    refuse_existing(db)?;
    for sidecar in sidecars(db) {
        refuse_existing(&sidecar)?;
    }
    let staging = Staging::beside(db)?;
    let staged = staging.file();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)
        .map_err(SqliteStoreError::io(&staged))?;
    File::open(&backup)
        .and_then(|mut source| std::io::copy(&mut source, &mut file))
        .and_then(|_| file.sync_all())
        .map_err(SqliteStoreError::io(&backup))?;
    let owner = StoreOwner::acquire_file(&config, file)?;
    validate(&staged, &backup).await?;
    if abandoned.load(Ordering::SeqCst) {
        return Err(abandoned_error(db));
    }
    publish(&staged, db)?;
    drop(staging);
    let store = open_published(owner, db).await?;
    if abandoned.load(Ordering::SeqCst) {
        let _ = store.close().await;
        return Err(abandoned_error(db));
    }
    Ok(store)
}

/// Open a just-published restore. On failure, remove what the restore
/// published, but only while the path still names the owned inode: an unknown
/// replacement is never unlinked.
pub(super) async fn open_published(
    owner: StoreOwner,
    db: &Path,
) -> Result<SqliteStore, SqliteStoreError> {
    match SqliteStore::open_owned(owner, db).await {
        Ok(store) => Ok(store),
        Err((error, owner)) => {
            remove_published(&owner, db);
            drop(owner);
            Err(error)
        }
    }
}

pub(super) fn remove_published(owner: &StoreOwner, db: &Path) {
    if owner.identity().path_is_owned() {
        for path in std::iter::once(db.to_owned()).chain(sidecars(db)) {
            let _ = fs::remove_file(path);
        }
    }
}
