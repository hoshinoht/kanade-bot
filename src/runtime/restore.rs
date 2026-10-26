//! `kanade restore --from SNAPSHOT [--identity FILE] [--apply]`: replace
//! the store with a `kanade backup` snapshot while serve is stopped.
//!
//! In order: the current store is opened through the owner lock (refused
//! while serve owns it, exit 69); an `.age` snapshot is decrypted into a
//! hidden 0700 staging directory beside the database; the copy is restored
//! there by the store itself (`integrity_check`, the migration ledger, a
//! schema no newer than this build, then this build's migrations) and
//! closed. Up to here nothing outside the staging directory is written, and
//! a dry run (no `--apply`) stops here. `--apply` then closes the current
//! store, keeps it beside the database as `<db>.pre-restore-<stamp>` (a hard
//! link, with any `-wal`/`-shm`/`-journal` renamed along), renames the
//! staged copy over the database and reopens it once; ownership of both
//! inodes is held from the close to the reopen, so a bot starting meanwhile
//! is refused (exit 69) instead of opening either. Store paths are never
//! followed through symbolic links (the store refuses them), nor is
//! `--from`.

use std::{
    fmt,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, ErrorKind},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};

use super::{
    backup::{AGE_SUFFIX, crypt::Identities, stamp},
    config::StoreSettings,
    error::Error,
    serve::store,
};
use crate::infrastructure::store::{SqliteStore, SqliteStoreConfig, SqliteStoreError};
use kanade_domain::history::{ChangeHistory, ChangeRef};

/// SQLite's companions of a database file.
const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub from: PathBuf,
    /// Required for an age-encrypted snapshot.
    pub identity: Option<PathBuf>,
    /// Without it, everything up to the swap runs and nothing is replaced.
    pub apply: bool,
}

/// What was checked and done, printed for the operator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub from: PathBuf,
    pub schema_version: i64,
    pub history_head: ChangeRef,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Dry run; `kept` is the side copy `--apply` would keep (`None`: no store yet).
    Checked { kept: Option<String> },
    /// Swapped in; `kept` names the previous store's side copy.
    Restored { kept: Option<String> },
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "restore {}: valid snapshot, schema {}, history head {} ({})",
            self.from.display(),
            self.schema_version,
            self.history_head.seq,
            self.history_head.hash
        )?;
        match &self.outcome {
            Outcome::Checked { kept: Some(kept) } => writeln!(
                f,
                "dry run: nothing changed; --apply replaces the store at KANADE_DB_PATH and keeps the current one as {kept}"
            ),
            Outcome::Checked { kept: None } => writeln!(
                f,
                "dry run: nothing changed; --apply puts it at KANADE_DB_PATH (no store there yet)"
            ),
            Outcome::Restored { kept: Some(kept) } => writeln!(
                f,
                "restored: KANADE_DB_PATH now holds the snapshot; the previous store is kept beside it as {kept}"
            ),
            Outcome::Restored { kept: None } => {
                writeln!(f, "restored: KANADE_DB_PATH now holds the snapshot")
            }
        }
    }
}

/// `<db>.pre-restore-20261011T120000Z`.
fn side_name(db: &Path, now: DateTime<Utc>) -> String {
    let name = db
        .file_name()
        .map_or_else(Default::default, |name| name.to_string_lossy().into_owned());
    format!("{name}.pre-restore-{}", stamp(now))
}

fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    let mut path = db.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

fn present(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::Unavailable(format!(
            "restore: {} cannot be checked ({})",
            path.display(),
            error.kind()
        ))),
    }
}

/// The hidden 0700 directory beside the database that holds the decrypted
/// and restored copies; removed on every exit path.
struct Staging(PathBuf);

impl Staging {
    fn beside(db: &Path) -> Result<Self, Error> {
        let name = db
            .file_name()
            .map_or_else(Default::default, |name| name.to_string_lossy().into_owned());
        let dir = db.with_file_name(format!(".{name}.restore-{}", uuid::Uuid::new_v4()));
        DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|error| {
                Error::Unavailable(format!(
                    "restore: the staging directory could not be created ({})",
                    error.kind()
                ))
            })?;
        Ok(Self(dir))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The snapshot restored and closed in staging.
struct Prepared {
    staging: Staging,
    db: PathBuf,
    schema_version: i64,
    history_head: ChangeRef,
}

fn current_error(error: SqliteStoreError) -> Error {
    match error {
        SqliteStoreError::Owned { .. } => Error::Unavailable(
            "restore: the store is owned by another process; stop the bot first".into(),
        ),
        SqliteStoreError::FutureVersion { found, known } => Error::Configuration(format!(
            "restore: the current store's schema {found} is newer than this build ({known}); restore with a newer image, or roll back with the pre-deploy tarball"
        )),
        error @ (SqliteStoreError::UnsafePath { .. }
        | SqliteStoreError::ChecksumMismatch { .. }
        | SqliteStoreError::MigrationGap { .. }) => {
            Error::Configuration(format!("restore: the current store: {error}"))
        }
        error => Error::Unavailable(format!("restore: the current store: {error}")),
    }
}

fn snapshot_error(error: SqliteStoreError) -> Error {
    match error {
        SqliteStoreError::InvalidBackup { detail, .. } => Error::Configuration(format!(
            "restore: --from is not a usable snapshot ({detail}); nothing changed"
        )),
        SqliteStoreError::FutureVersion { found, known } => Error::Configuration(format!(
            "restore: --from has schema {found}, newer than this build ({known}); restore it with a newer image; nothing changed"
        )),
        error @ (SqliteStoreError::ChecksumMismatch { .. }
        | SqliteStoreError::MigrationGap { .. }
        | SqliteStoreError::Database(_)) => Error::Configuration(format!(
            "restore: --from is not a usable snapshot ({error}); nothing changed"
        )),
        error => Error::Unavailable(format!("restore: {error}; nothing changed")),
    }
}

/// Restore `options.from` over the store in `settings` (see the module
/// docs); `now` stamps the side copy's name.
///
/// # Errors
/// Usage (64) for an encrypted `--from` without `--identity`; 69 while
/// another process owns the store; 78 for an unreadable or unusable
/// snapshot, a bad identity or an unsafe path. Every refusal before the
/// swap leaves the store untouched.
pub async fn run(
    options: &Options,
    settings: &StoreSettings,
    now: DateTime<Utc>,
) -> Result<Report, Error> {
    let from = &options.from;
    match fs::symlink_metadata(from) {
        Ok(meta) if meta.is_file() => {}
        _ => {
            return Err(Error::Configuration(
                "restore: --from must name a regular file (symbolic links are refused)".into(),
            ));
        }
    }
    if options.identity.is_none() && from.to_string_lossy().ends_with(AGE_SUFFIX) {
        return Err(Error::Usage(
            "restore: --from is age-encrypted; pass --identity FILE".into(),
        ));
    }
    let identities = options
        .identity
        .as_deref()
        .map(|path| Identities::load(path, "restore: --identity"))
        .transpose()?;
    let config = store::prepare(settings)?;
    let db = &config.db_path;

    // Ownership first, so a running bot refuses the restore before
    // anything is staged. `open` would create a missing store, so a missing
    // one is only checked for stray journal files.
    let current = if present(db)? {
        Some(SqliteStore::open(&config).await.map_err(current_error)?)
    } else {
        for suffix in SIDECARS {
            if present(&sidecar(db, suffix))? {
                return Err(Error::Configuration(format!(
                    "restore: KANADE_DB_PATH has a {suffix} file but no database; move it aside first"
                )));
            }
        }
        None
    };
    let kept = current.is_some().then(|| side_name(db, now));
    let prepared = match prepare(from, identities, &config).await {
        Ok(prepared) => prepared,
        Err(error) => {
            close(current).await?;
            return Err(error);
        }
    };
    let mut report = Report {
        from: from.clone(),
        schema_version: prepared.schema_version,
        history_head: prepared.history_head.clone(),
        outcome: Outcome::Checked { kept: kept.clone() },
    };
    if !options.apply {
        close(current).await?;
        return Ok(report);
    }
    let side = kept.as_ref().map(|name| db.with_file_name(name));
    if let Some(side) = &side {
        for path in std::iter::once(side.clone()).chain(SIDECARS.map(|s| sidecar(side, s))) {
            if present(&path)? {
                close(current).await?;
                return Err(Error::Configuration(format!(
                    "restore: {} already exists; try again in a second",
                    path.display()
                )));
            }
        }
    }
    // Closing checkpoints the WAL into the database file but keeps owning
    // its inode, and the copy's inode is owned too: from here to the reopen
    // a starting bot is refused (exit 69) whichever one the path names.
    let held = SqliteStore::hold_for_swap(&config, current, &prepared.db)
        .await
        .map_err(current_error)?;
    swap(db, &prepared.db, side.as_deref())?;
    drop(prepared.staging);
    // Reopen once, still owned: the swapped-in store opens.
    let reopened = held.open().await.map_err(|error| {
        let error = current_error(error);
        Error::Unavailable(format!(
            "{error} (after the swap: the snapshot is in place{})",
            kept.as_ref()
                .map(|kept| format!(", the previous store is kept as {kept}"))
                .unwrap_or_default()
        ))
    })?;
    reopened
        .close()
        .await
        .map_err(|error| Error::Unavailable(format!("restore: closing the store: {error}")))?;
    report.outcome = Outcome::Restored { kept };
    Ok(report)
}

async fn close(store: Option<SqliteStore>) -> Result<(), Error> {
    match store {
        Some(store) => store
            .close()
            .await
            .map_err(|error| Error::Unavailable(format!("restore: closing the store: {error}"))),
        None => Ok(()),
    }
}

/// Decrypt (when an identity is given) and restore `from` in a fresh
/// staging directory, then close it there.
async fn prepare(
    from: &Path,
    identities: Option<Identities>,
    config: &SqliteStoreConfig,
) -> Result<Prepared, Error> {
    let staging = Staging::beside(&config.db_path)?;
    let source = match identities {
        None => from.to_owned(),
        Some(identities) => {
            let plain = staging.0.join("snapshot.sqlite");
            let (sealed, out) = (from.to_owned(), plain.clone());
            tokio::task::spawn_blocking(move || decrypt(&identities, &sealed, &out))
                .await
                .map_err(|error| Error::Unavailable(format!("restore: decrypt: {error}")))??;
            plain
        }
    };
    let name = config
        .db_path
        .file_name()
        .ok_or_else(|| Error::Configuration("KANADE_DB_PATH must name a file".into()))?;
    let staged = SqliteStoreConfig {
        db_path: staging.0.join(name),
        owner_lock_dir: config.owner_lock_dir.clone(),
    };
    let store = SqliteStore::restore(&source, &staged)
        .await
        .map_err(snapshot_error)?;
    let facts = async {
        Ok::<_, String>((
            store.schema_version().await.map_err(|e| e.to_string())?,
            store.history_head().await.map_err(|e| e.to_string())?,
        ))
    }
    .await;
    let closed = store.close().await;
    let (schema_version, history_head) =
        facts.map_err(|error| Error::Unavailable(format!("restore: reading the copy: {error}")))?;
    closed.map_err(|error| Error::Unavailable(format!("restore: closing the copy: {error}")))?;
    // A clean close leaves no journal files; one left behind holds data the
    // database file lacks, so the copy is not swapped in.
    for suffix in SIDECARS {
        if present(&sidecar(&staged.db_path, suffix))? {
            return Err(Error::Unavailable(format!(
                "restore: the restored copy left a {suffix} file; nothing changed"
            )));
        }
    }
    if source != from {
        let _ = fs::remove_file(&source);
    }
    Ok(Prepared {
        staging,
        db: staged.db_path,
        schema_version,
        history_head,
    })
}

/// Decrypt the age file `from` into a new 0600 `out`, never following a
/// symbolic link at either end.
fn decrypt(identities: &Identities, from: &Path, out: &Path) -> Result<(), Error> {
    let sealed = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(from)
        .map_err(|error| {
            Error::Configuration(format!(
                "restore: --from could not be read ({})",
                error.kind()
            ))
        })?;
    let mut plain = identities
        .decrypt(sealed)
        .map_err(|error| Error::Configuration(format!("restore: --from: {error}")))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(out)
        .map_err(|error| {
            Error::Unavailable(format!(
                "restore: the staged copy could not be created ({})",
                error.kind()
            ))
        })?;
    io::copy(&mut plain, &mut file)
        .and_then(|_| file.sync_all())
        .map_err(|error| match error.kind() {
            ErrorKind::InvalidData | ErrorKind::UnexpectedEof => Error::Configuration(
                "restore: --from is damaged or truncated; nothing changed".into(),
            ),
            kind => Error::Unavailable(format!(
                "restore: the staged copy could not be written ({kind})"
            )),
        })
}

/// Keep the closed current database as `side` (a hard link, its journal
/// files renamed along), then rename the staged copy over `db`, which
/// always names a whole database. With no current store the copy is
/// linked in, never over a file that appeared meanwhile.
fn swap(db: &Path, staged: &Path, side: Option<&Path>) -> Result<(), Error> {
    let failed = |step: &str, error: io::Error| {
        Error::Unavailable(format!("restore: {step} failed ({})", error.kind()))
    };
    #[cfg(test)]
    tests::swap_step(db);
    match side {
        Some(side) => {
            fs::hard_link(db, side).map_err(|error| failed("keeping the current store", error))?;
            for suffix in SIDECARS {
                let from = sidecar(db, suffix);
                if present(&from)? {
                    fs::rename(&from, sidecar(side, suffix))
                        .map_err(|error| failed("keeping the current store's journal", error))?;
                }
            }
            #[cfg(test)]
            tests::swap_step(db);
            fs::rename(staged, db).map_err(|error| failed("swapping the snapshot in", error))?;
        }
        None => {
            fs::hard_link(staged, db).map_err(|error| failed("putting the snapshot in", error))?;
        }
    }
    if let Some(dir) = db.parent() {
        File::open(dir)
            .and_then(|dir| dir.sync_all())
            .map_err(|error| failed("syncing the store directory", error))?;
    }
    #[cfg(test)]
    tests::swap_step(db);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, fs::TryLockError, os::unix::fs::MetadataExt, rc::Rc};

    use chrono::TimeZone;

    use super::*;

    type SwapStep = Box<dyn FnMut(&Path)>;

    thread_local! {
        /// Called with the database path at each step of the swap.
        static SWAP_STEP: RefCell<Option<SwapStep>> = const { RefCell::new(None) };
    }

    pub(super) fn swap_step(db: &Path) {
        SWAP_STEP.with_borrow_mut(|step| {
            if let Some(step) = step {
                step(db);
            }
        });
    }

    #[test]
    fn the_side_copy_is_named_after_the_database_and_the_stamp() {
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 7, 9, 3).unwrap();
        assert_eq!(
            side_name(Path::new("/data/db/kanade.sqlite"), now),
            "kanade.sqlite.pre-restore-20261011T070903Z"
        );
        assert_eq!(
            sidecar(Path::new("/data/db/kanade.sqlite"), "-wal"),
            PathBuf::from("/data/db/kanade.sqlite-wal")
        );
    }

    /// At every step of the swap, whichever inode the database path names
    /// is locked (a second process's `flock` would block) and serve's own
    /// open is refused as owned, so it never opens the database mid-swap.
    #[tokio::test]
    async fn an_owner_starting_during_the_swap_is_refused() {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("kanade-restore-swap-{}", uuid::Uuid::new_v4()));
        let settings = StoreSettings {
            db_path: root.join("db/kanade.sqlite"),
            owner_lock_dir: root.join("locks"),
        };
        for dir in [root.join("db"), root.join("locks")] {
            DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(dir)
                .unwrap();
        }
        let config = SqliteStoreConfig {
            db_path: settings.db_path.clone(),
            owner_lock_dir: settings.owner_lock_dir.clone(),
        };
        let snapshot = SqliteStoreConfig {
            db_path: root.join("db/snapshot.sqlite"),
            owner_lock_dir: settings.owner_lock_dir.clone(),
        };
        for config in [&config, &snapshot] {
            SqliteStore::open(config)
                .await
                .unwrap()
                .close()
                .await
                .unwrap();
        }
        let before = fs::metadata(&config.db_path).unwrap().ino();

        let seen = Rc::new(RefCell::new(Vec::new()));
        let (log, lock_dir, contender) = (
            Rc::clone(&seen),
            settings.owner_lock_dir.clone(),
            config.clone(),
        );
        SWAP_STEP.set(Some(Box::new(move |db: &Path| {
            let meta = fs::metadata(db).unwrap();
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .open(lock_dir.join(format!("kanade-{:x}-{:x}.lock", meta.dev(), meta.ino())))
                .unwrap();
            assert!(
                matches!(lock.try_lock(), Err(TryLockError::WouldBlock)),
                "the inode at the database path is not locked"
            );
            let contender = contender.clone();
            let refused = std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(SqliteStore::open(&contender))
                    .err()
            })
            .join()
            .unwrap();
            assert!(
                matches!(refused, Some(SqliteStoreError::Owned { .. })),
                "{refused:?}"
            );
            log.borrow_mut().push(meta.ino());
        })));
        let report = run(
            &Options {
                from: snapshot.db_path.clone(),
                identity: None,
                apply: true,
            },
            &settings,
            Utc.with_ymd_and_hms(2026, 10, 11, 7, 9, 3).unwrap(),
        )
        .await;
        SWAP_STEP.set(None);
        let report = report.unwrap();
        assert_eq!(
            report.outcome,
            Outcome::Restored {
                kept: Some("kanade.sqlite.pre-restore-20261011T070903Z".into())
            }
        );
        let after = fs::metadata(&config.db_path).unwrap().ino();
        assert_ne!(before, after);
        // Before the swap, with the side copy kept, and after the rename.
        assert_eq!(*seen.borrow(), [before, before, after]);
        // Released once restore is done.
        SqliteStore::open(&config)
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
