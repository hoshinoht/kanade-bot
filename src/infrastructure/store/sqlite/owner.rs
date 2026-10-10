//! Single-writer ownership of one database inode (maintenance contract,
//! "One writable Repo owns each persistent SQLite inode").
//!
//! The database file is opened (created if absent) and kept open first; its
//! `(device, inode)` then names a lockfile in the operator-configured private
//! owner lock directory, which is `flock`ed nonblocking and exclusive. The
//! database inode itself is never locked (Darwin whole-file `flock` conflicts
//! with SQLite; POSIX byte locks vanish when any descriptor for the inode
//! closes). Hard links and other paths to the same database contend on the
//! same lockfile, and a process-local registry refuses a second owner in this
//! process before the kernel lock is even tried.
//!
//! Paths must be absolute and free of `..` and symbolic links (on macOS use
//! `/private/var`, not `/var`). This is a same-UID cooperating-process
//! guarantee: components are checked, then opened no-follow, but a hostile
//! same-UID actor can still race the intermediate directories.
//!
//! The lock lives as long as its descriptor: released on drop and by the
//! kernel when the process dies. Lockfiles are never truncated or removed.

use std::collections::BTreeSet;
use std::fs::{self, File, Metadata, OpenOptions, TryLockError};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use super::{SqliteStoreConfig, SqliteStoreError};

const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

/// Inodes owned by this process.
static OWNED: Mutex<BTreeSet<(u64, u64)>> = Mutex::new(BTreeSet::new());

fn effective_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments, has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn unsafe_path(path: &Path, reason: &'static str) -> SqliteStoreError {
    SqliteStoreError::UnsafePath {
        path: path.to_owned(),
        reason,
    }
}

/// The lockfile name for a database `(device, inode)`.
pub(super) fn lock_file_name(device: u64, inode: u64) -> String {
    format!("kanade-{device:x}-{inode:x}.lock")
}

/// An absolute, `..`-free path none of whose existing components is a
/// symbolic link.
fn check_components(path: &Path) -> Result<(), SqliteStoreError> {
    if !path.is_absolute() {
        return Err(unsafe_path(path, "must be an absolute path"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => return Err(unsafe_path(path, "must not contain `..`")),
            Component::CurDir => continue,
            other => current.push(other),
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(unsafe_path(&current, "is a symbolic link"));
            }
            Ok(_) => {}
            Err(error) => return Err(SqliteStoreError::io(&current)(error)),
        }
    }
    Ok(())
}

fn check_owner(path: &Path, meta: &Metadata) -> Result<(), SqliteStoreError> {
    if meta.uid() == effective_uid() {
        Ok(())
    } else {
        Err(unsafe_path(path, "is not owned by this user"))
    }
}

/// How private a directory must be.
#[derive(Clone, Copy)]
enum DirMode {
    /// Exactly 0700 (the owner lock directory).
    Exact,
    /// No group or other permissions (0700 or stricter).
    AtMostPrivate,
}

/// Open a private directory no-follow and check type, owner and mode.
fn open_private_dir(path: &Path, mode: DirMode) -> Result<File, SqliteStoreError> {
    check_components(path)?;
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| match error.raw_os_error() {
            Some(libc::ELOOP) => unsafe_path(path, "is a symbolic link"),
            Some(libc::ENOTDIR) => unsafe_path(path, "is not a directory"),
            _ => SqliteStoreError::io(path)(error),
        })?;
    let meta = dir.metadata().map_err(SqliteStoreError::io(path))?;
    if !meta.is_dir() {
        return Err(unsafe_path(path, "is not a directory"));
    }
    check_owner(path, &meta)?;
    let bits = meta.permissions().mode() & 0o777;
    let private = match mode {
        DirMode::Exact => bits == PRIVATE_DIR_MODE,
        DirMode::AtMostPrivate => bits & 0o077 == 0,
    };
    if !private {
        return Err(unsafe_path(
            path,
            match mode {
                DirMode::Exact => "must have mode 0700",
                DirMode::AtMostPrivate => "must not be accessible to group or others (0700)",
            },
        ));
    }
    Ok(dir)
}

fn same_inode(a: &Metadata, b: &Metadata) -> bool {
    (a.dev(), a.ino()) == (b.dev(), b.ino())
}

/// The owner lock directory and the database's directory, both private.
/// Returns the lock directory handle for the lockfile open.
pub(super) fn check_dirs(config: &SqliteStoreConfig) -> Result<File, SqliteStoreError> {
    let lock_dir = open_private_dir(&config.owner_lock_dir, DirMode::Exact)?;
    let db_dir = config
        .db_path
        .parent()
        .filter(|_| config.db_path.file_name().is_some())
        .ok_or_else(|| unsafe_path(&config.db_path, "must name a file"))?;
    open_private_dir(db_dir, DirMode::AtMostPrivate)?;
    Ok(lock_dir)
}

/// A database file this user owns with mode 0600 or stricter.
fn check_db_file(path: &Path, file: &File) -> Result<(), SqliteStoreError> {
    let meta = file.metadata().map_err(SqliteStoreError::io(path))?;
    if !meta.is_file() {
        return Err(unsafe_path(path, "is not a regular file"));
    }
    check_owner(path, &meta)?;
    if meta.permissions().mode() & 0o177 != 0 {
        return Err(unsafe_path(path, "must have mode 0600"));
    }
    Ok(())
}

/// The retained database handle and the path it must stay reachable at.
#[derive(Debug)]
pub(super) struct DbIdentity {
    path: PathBuf,
    file: File,
    key: (u64, u64),
}

impl DbIdentity {
    /// The configured path itself (not a symlink, even to the owned file)
    /// still names the retained inode.
    ///
    /// # Errors
    /// [`SqliteStoreError::IdentityChanged`] when the file was removed or
    /// replaced, so writes would go to an unowned database.
    pub(super) fn verify(&self) -> Result<(), SqliteStoreError> {
        if self.path_is_owned() {
            Ok(())
        } else {
            Err(SqliteStoreError::IdentityChanged {
                db_path: self.path.clone(),
            })
        }
    }

    /// Whether the path's own directory entry is the retained regular file.
    pub(super) fn path_is_owned(&self) -> bool {
        let Ok(retained) = self.file.metadata() else {
            return false;
        };
        let Ok(current) = fs::symlink_metadata(&self.path) else {
            return false;
        };
        retained.is_file()
            && current.is_file()
            && (retained.dev(), retained.ino()) == self.key
            && same_inode(&retained, &current)
    }
}

/// Exclusive ownership of one database inode for this store's lifetime.
#[derive(Debug)]
pub(super) struct StoreOwner {
    identity: Arc<DbIdentity>,
    // Held only for its lock; closing it releases the flock.
    _lock: File,
}

impl StoreOwner {
    /// Check the directories, open (or create) the database file, then lock.
    ///
    /// # Errors
    /// [`SqliteStoreError::UnsafePath`] for any unsafe directory or file,
    /// [`SqliteStoreError::Owned`] while another owner holds the inode.
    pub(super) fn acquire(config: &SqliteStoreConfig) -> Result<Self, SqliteStoreError> {
        let lock_dir = check_dirs(config)?;
        let path = &config.db_path;
        if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(unsafe_path(path, "is a symbolic link"));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(PRIVATE_FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| match error.raw_os_error() {
                Some(libc::ELOOP) => unsafe_path(path, "is a symbolic link"),
                _ => SqliteStoreError::io(path)(error),
            })?;
        Self::lock(config, &lock_dir, file)
    }

    /// Own an already-open database file (a restore's staged copy, whose
    /// inode is later published at `config.db_path`).
    pub(super) fn acquire_file(
        config: &SqliteStoreConfig,
        file: File,
    ) -> Result<Self, SqliteStoreError> {
        let lock_dir = check_dirs(config)?;
        Self::lock(config, &lock_dir, file)
    }

    fn lock(
        config: &SqliteStoreConfig,
        lock_dir: &File,
        file: File,
    ) -> Result<Self, SqliteStoreError> {
        let db_path = &config.db_path;
        check_db_file(db_path, &file)?;
        let meta = file.metadata().map_err(SqliteStoreError::io(db_path))?;
        let key = (meta.dev(), meta.ino());
        let owned_error = || SqliteStoreError::Owned {
            db_path: db_path.clone(),
        };
        let mut owned = OWNED.lock().unwrap_or_else(PoisonError::into_inner);
        if owned.contains(&key) {
            return Err(owned_error());
        }
        let lock_path = config.owner_lock_dir.join(lock_file_name(key.0, key.1));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(PRIVATE_FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&lock_path)
            .map_err(|error| match error.raw_os_error() {
                Some(libc::ELOOP) => unsafe_path(&lock_path, "is a symbolic link"),
                _ => SqliteStoreError::io(&lock_path)(error),
            })?;
        let lock_meta = lock.metadata().map_err(SqliteStoreError::io(&lock_path))?;
        if !lock_meta.is_file() {
            return Err(unsafe_path(&lock_path, "is not a regular file"));
        }
        check_owner(&lock_path, &lock_meta)?;
        if lock_meta.permissions().mode() & 0o777 != PRIVATE_FILE_MODE {
            return Err(unsafe_path(&lock_path, "must have mode 0600"));
        }
        if lock_meta.nlink() != 1 {
            return Err(unsafe_path(&lock_path, "must have exactly one link"));
        }
        // The directory checked is still the one at the configured path.
        let dir_now = fs::symlink_metadata(&config.owner_lock_dir)
            .map_err(SqliteStoreError::io(&config.owner_lock_dir))?;
        let dir_then = lock_dir
            .metadata()
            .map_err(SqliteStoreError::io(&config.owner_lock_dir))?;
        if !same_inode(&dir_now, &dir_then) {
            return Err(unsafe_path(&config.owner_lock_dir, "was replaced"));
        }
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(owned_error()),
            Err(TryLockError::Error(source)) => {
                return Err(SqliteStoreError::Io {
                    path: lock_path,
                    source,
                });
            }
        }
        owned.insert(key);
        Ok(Self {
            identity: Arc::new(DbIdentity {
                path: db_path.clone(),
                file,
                key,
            }),
            _lock: lock,
        })
    }

    pub(super) fn identity(&self) -> Arc<DbIdentity> {
        Arc::clone(&self.identity)
    }
}

impl Drop for StoreOwner {
    fn drop(&mut self) {
        OWNED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.identity.key);
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{DirBuilderExt, symlink};

    use super::*;

    /// A canonical private temp directory, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            let root = fs::canonicalize(std::env::temp_dir()).expect("temp dir");
            let path = root.join(format!("kanade-owner-{}", uuid::Uuid::new_v4()));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .expect("create");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path.join("locks"))
                .expect("create");
            Self(path)
        }

        fn config(&self) -> SqliteStoreConfig {
            SqliteStoreConfig {
                db_path: self.0.join("db.sqlite3"),
                owner_lock_dir: self.0.join("locks"),
            }
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn reason(error: SqliteStoreError) -> &'static str {
        match error {
            SqliteStoreError::UnsafePath { reason, .. } => reason,
            other => panic!("expected an unsafe path: {other}"),
        }
    }

    #[test]
    fn lockfile_is_named_by_device_and_inode() {
        assert_eq!(lock_file_name(0x2a, 0xbeef), "kanade-2a-beef.lock");
        let dir = Dir::new();
        let owner = StoreOwner::acquire(&dir.config()).expect("acquire");
        let meta = fs::metadata(dir.config().db_path).expect("db exists");
        let lock = dir
            .0
            .join("locks")
            .join(lock_file_name(meta.dev(), meta.ino()));
        let lock_meta = fs::symlink_metadata(&lock).expect("lockfile exists");
        assert_eq!(lock_meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(meta.permissions().mode() & 0o777, 0o600, "db created 0600");
        drop(owner);
        assert!(lock.exists(), "never removed");
    }

    #[test]
    fn relative_and_dotdot_paths_are_refused() {
        let dir = Dir::new();
        let mut config = dir.config();
        config.owner_lock_dir = PathBuf::from("locks");
        assert_eq!(
            reason(StoreOwner::acquire(&config).unwrap_err()),
            "must be an absolute path"
        );
        config.owner_lock_dir = dir.0.join("locks/../locks");
        assert_eq!(
            reason(StoreOwner::acquire(&config).unwrap_err()),
            "must not contain `..`"
        );
    }

    #[test]
    fn a_lockfile_with_another_link_is_refused() {
        let dir = Dir::new();
        let config = dir.config();
        drop(StoreOwner::acquire(&config).expect("acquire"));
        let meta = fs::metadata(&config.db_path).expect("db");
        let lock = config
            .owner_lock_dir
            .join(lock_file_name(meta.dev(), meta.ino()));
        fs::hard_link(&lock, dir.0.join("alias.lock")).expect("link");
        assert_eq!(
            reason(StoreOwner::acquire(&config).unwrap_err()),
            "must have exactly one link"
        );
    }

    #[test]
    fn a_symlinked_lockfile_is_refused() {
        let dir = Dir::new();
        let config = dir.config();
        drop(StoreOwner::acquire(&config).expect("acquire"));
        let meta = fs::metadata(&config.db_path).expect("db");
        let lock = config
            .owner_lock_dir
            .join(lock_file_name(meta.dev(), meta.ino()));
        fs::remove_file(&lock).expect("remove");
        fs::write(dir.0.join("elsewhere.lock"), b"").expect("target");
        symlink(dir.0.join("elsewhere.lock"), &lock).expect("symlink");
        assert_eq!(
            reason(StoreOwner::acquire(&config).unwrap_err()),
            "is a symbolic link"
        );
    }

    /// Move the database aside and put a private copy at its path.
    fn swap(config: &SqliteStoreConfig) -> PathBuf {
        let moved = config.db_path.with_file_name("moved.sqlite3");
        fs::rename(&config.db_path, &moved).expect("move");
        fs::copy(&moved, &config.db_path).expect("impostor");
        fs::set_permissions(&config.db_path, fs::Permissions::from_mode(0o600)).expect("chmod");
        moved
    }

    #[test]
    fn a_lock_dir_replaced_after_its_check_is_refused() {
        let dir = Dir::new();
        let config = dir.config();
        let lock_dir = check_dirs(&config).expect("dirs");
        fs::rename(&config.owner_lock_dir, dir.0.join("locks-old")).expect("move");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&config.owner_lock_dir)
            .expect("replacement");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&config.db_path)
            .expect("db");
        assert_eq!(
            reason(StoreOwner::lock(&config, &lock_dir, file).unwrap_err()),
            "was replaced"
        );
    }

    #[test]
    fn a_symlink_even_to_the_owned_file_fails_verification() {
        let dir = Dir::new();
        let config = dir.config();
        let owner = StoreOwner::acquire(&config).expect("acquire");
        let identity = owner.identity();
        identity.verify().expect("owned");
        let moved = config.db_path.with_file_name("moved.sqlite3");
        fs::rename(&config.db_path, &moved).expect("move");
        symlink(&moved, &config.db_path).expect("symlink");
        assert!(matches!(
            identity.verify(),
            Err(SqliteStoreError::IdentityChanged { .. })
        ));
    }

    #[tokio::test]
    async fn opening_refuses_a_path_swapped_after_ownership() {
        let dir = Dir::new();
        let config = dir.config();
        let owner = StoreOwner::acquire(&config).expect("acquire");
        swap(&config);
        let refused = super::super::SqliteStore::open_owned(owner, &config.db_path).await;
        assert!(matches!(
            refused,
            Err((SqliteStoreError::IdentityChanged { .. }, _))
        ));
    }

    #[tokio::test]
    async fn a_failed_restore_never_unlinks_an_unknown_replacement() {
        let dir = Dir::new();
        let config = dir.config();
        // Stands in for a just-published restore whose path was then swapped.
        let owner = StoreOwner::acquire(&config).expect("acquire");
        swap(&config);
        let wal = config.db_path.with_file_name("db.sqlite3-wal");
        fs::write(&wal, b"").expect("sidecar");
        let refused = super::super::backup::open_published(owner, &config.db_path).await;
        assert!(matches!(
            refused,
            Err(SqliteStoreError::IdentityChanged { .. })
        ));
        assert!(config.db_path.exists(), "the replacement is kept");
        assert!(wal.exists(), "its sidecars are kept");

        // When the path still names the owned file, what was published goes.
        fs::remove_file(&config.db_path).expect("remove impostor");
        fs::remove_file(&wal).expect("remove sidecar");
        let owner = StoreOwner::acquire(&config).expect("acquire");
        fs::write(&wal, b"").expect("sidecar");
        super::super::backup::remove_published(&owner, &config.db_path);
        assert!(!config.db_path.exists() && !wal.exists());
    }
}
