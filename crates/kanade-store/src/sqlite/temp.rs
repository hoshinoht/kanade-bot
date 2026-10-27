//! Test support: stores on their own private temp directory, removed once
//! the store is closed or dropped. Each starts as a copy of one migrated
//! template, since migrating from scratch costs ~75 ms a store.

use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};
use tokio::sync::OnceCell;

use super::{SqliteCharData, SqliteStore, SqliteStoreConfig, migrate};
use kanade_domain::history::ChangeRecord;

fn temp_root() -> PathBuf {
    std::fs::canonicalize(std::env::temp_dir()).expect("temp dir resolves")
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

/// The migrated, closed store every temp store copies, one per
/// [`template_key`] in a private directory shared by every test process of
/// this user. It is built in a fresh directory and renamed into place, so a
/// reader never sees a partial file; racing builders publish identical
/// files.
async fn template() -> &'static Path {
    static TEMPLATE: OnceCell<PathBuf> = OnceCell::const_new();
    TEMPLATE.get_or_init(build_template).await
}

/// Names what a fresh store holds: every embedded migration and the genesis
/// record a fresh store writes (at revision 0, as `0001_init` leaves it), so
/// a change to either builds a new template.
fn template_key() -> String {
    let genesis = ChangeRecord::genesis(0).expect("genesis record");
    let canonical = genesis.canonical().expect("genesis body");
    migrate::checksum(&format!(
        "{}\n{canonical}\n{}",
        migrate::ledger_hash(),
        genesis.hash
    ))
}

/// Why `root` cannot hold templates: it must be a directory (not a
/// symlink) owned by this user with no group or other access.
fn unsafe_template_dir(root: &Path) -> Option<String> {
    let meta = match std::fs::symlink_metadata(root) {
        Ok(meta) => meta,
        Err(error) => return Some(format!("cannot be read: {error}")),
    };
    // SAFETY: `geteuid` takes no arguments and cannot fail.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_dir() {
        Some("is not a directory".into())
    } else if meta.uid() != uid {
        Some(format!("is owned by uid {}, not {uid}", meta.uid()))
    } else if meta.mode() & 0o077 != 0 {
        Some(format!("has mode {:o}, not 0700", meta.mode() & 0o777))
    } else {
        None
    }
}

async fn build_template() -> PathBuf {
    let root = temp_root().join("kanade-store-templates");
    if let Err(error) = private_dir(&root)
        && error.kind() != std::io::ErrorKind::AlreadyExists
    {
        panic!(
            "cannot create the store template dir {}: {error}",
            root.display()
        );
    }
    if let Some(reason) = unsafe_template_dir(&root) {
        panic!(
            "store template dir {} {reason}; remove it (it holds only test templates) and rerun",
            root.display()
        );
    }
    let path = root.join(format!("{}.sqlite3", template_key()));
    if path.exists() {
        return path;
    }
    let staging = TempStoreDir::create();
    let config = staging.config();
    let store = SqliteStore::open(&config).await.expect("template opens");
    store.close().await.expect("template closes");
    // Closing checkpoints the WAL away; a copy without it would be partial.
    let wal = config.db_path.with_extension("sqlite3-wal");
    assert!(!wal.exists(), "template left {}", wal.display());
    std::fs::rename(&config.db_path, &path).expect("template is published");
    path
}

/// A fresh directory under the canonical temp dir (the store refuses
/// symlinked paths, and macOS `/var` is one), 0700, with a 0700 `locks/`
/// owner lock directory, holding one database. Removed with its contents
/// on drop.
#[derive(Debug)]
pub struct TempStoreDir(PathBuf);

impl TempStoreDir {
    /// # Panics
    /// The temp dir cannot be resolved or the directories created.
    #[must_use]
    pub fn create() -> Self {
        let dir = Self(temp_root().join(format!("kanade-store-{}", uuid::Uuid::new_v4())));
        private_dir(&dir.0).expect("temp store dir is created");
        private_dir(&dir.0.join("locks")).expect("temp lock dir is created");
        dir
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The config of this directory's database.
    #[must_use]
    pub fn config(&self) -> SqliteStoreConfig {
        SqliteStoreConfig {
            db_path: self.0.join("store.sqlite3"),
            owner_lock_dir: self.0.join("locks"),
        }
    }

    /// Run `sql` on the database over a plain connection, bypassing the
    /// store (to tamper with rows a store API refuses to write).
    ///
    /// # Errors
    /// Connecting or the SQL failed.
    pub async fn raw(&self, sql: &str) -> Result<(), sqlx::Error> {
        let mut conn = SqliteConnectOptions::new()
            .filename(self.config().db_path)
            .connect()
            .await?;
        let result = sqlx::raw_sql(sql).execute(&mut conn).await.map(|_| ());
        conn.close().await?;
        result
    }
}

impl Drop for TempStoreDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl SqliteStore {
    /// A migrated store on its own [`TempStoreDir`], removed after
    /// ownership is released on [`Self::close`] or drop. Dropping it
    /// unclosed logs nothing. Not for `hold_for_swap`/restore, which
    /// reopen without the directory.
    ///
    /// # Panics
    /// The directory or the store cannot be created.
    pub async fn open_temp() -> Self {
        let dir = TempStoreDir::create();
        // A copy, never a hard link: the store refuses a file with two links.
        std::fs::copy(template().await, dir.config().db_path).expect("template is copied");
        let mut store = Self::open(&dir.config()).await.expect("temp store opens");
        store.unclosed.armed = false;
        store.temp_dir = Some(dir);
        store
    }

    /// The directory of a store from [`Self::open_temp`].
    ///
    /// # Panics
    /// The store was not opened by [`Self::open_temp`].
    #[must_use]
    pub fn temp_dir(&self) -> &TempStoreDir {
        self.temp_dir.as_ref().expect("a store from open_temp")
    }
}

impl SqliteCharData {
    /// A character-data file on its own [`TempStoreDir`], removed after
    /// ownership is released on [`Self::close`] or drop.
    ///
    /// # Panics
    /// The directory or the file cannot be created.
    pub async fn open_temp() -> Self {
        let dir = TempStoreDir::create();
        let mut store = Self::open(&dir.config())
            .await
            .expect("temp chardata opens");
        store.temp_dir = Some(dir);
        store
    }

    /// The directory of a file from [`Self::open_temp`].
    ///
    /// # Panics
    /// The file was not opened by [`Self::open_temp`].
    #[must_use]
    pub fn temp_dir(&self) -> &TempStoreDir {
        self.temp_dir
            .as_ref()
            .expect("a chardata file from open_temp")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_temp_store_is_private_separate_and_removed_after_close() {
        use std::os::unix::fs::PermissionsExt;

        let a = SqliteStore::open_temp().await;
        let b = SqliteStore::open_temp().await;
        let dir = a.temp_dir().path().to_owned();
        assert_ne!(dir, b.temp_dir().path());
        assert_eq!(dir, std::fs::canonicalize(&dir).expect("canonical"));
        for path in [dir.clone(), dir.join("locks")] {
            let mode = std::fs::metadata(&path)
                .expect("exists")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "{}", path.display());
        }
        a.temp_dir()
            .raw("INSERT INTO config (key, value) VALUES ('temp.probe', '1')")
            .await
            .expect("raw write");
        assert_eq!(
            a.schema_version().await.expect("version"),
            b.schema_version().await.expect("version")
        );
        a.close().await.expect("close");
        assert!(!dir.exists(), "removed after close");
        let other = b.temp_dir().path().to_owned();
        drop(b);
        assert!(!other.exists(), "removed after drop");
    }

    #[tokio::test]
    async fn a_temp_chardata_file_is_removed_after_close() {
        let store = SqliteCharData::open_temp().await;
        let dir = store.temp_dir().path().to_owned();
        assert!(store.temp_dir().config().db_path.exists());
        store.close().await.expect("close");
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn a_temp_store_is_its_own_copy_of_the_migrated_template() {
        use kanade_domain::history::ChangeHistory;
        use std::os::unix::fs::MetadataExt;

        let store = SqliteStore::open_temp().await;
        let db = store.temp_dir().config().db_path;
        assert_eq!(std::fs::metadata(&db).expect("db").nlink(), 1);
        assert_eq!(
            std::fs::metadata(template().await)
                .expect("template")
                .nlink(),
            1
        );
        let dir = TempStoreDir::create();
        let fresh = SqliteStore::open(&dir.config()).await.expect("fresh");
        assert_eq!(
            store.schema_version().await.expect("version"),
            fresh.schema_version().await.expect("version")
        );
        assert_eq!(store.foreign_key_violations().await.expect("fk"), 0);
        // A stale template (genesis code changed, no new migration) shows
        // here: its genesis differs from the one a fresh store writes, which
        // is the one the template key names.
        let head = store.history_head().await.expect("copy head");
        assert_eq!(head, fresh.history_head().await.expect("fresh head"));
        assert_eq!(head.hash, ChangeRecord::genesis(0).expect("genesis").hash);
        fresh.close().await.expect("close");
        store.close().await.expect("close");
    }

    #[test]
    fn a_shared_template_dir_must_be_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempStoreDir::create();
        assert_eq!(unsafe_template_dir(dir.path()), None);
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
        assert_eq!(
            unsafe_template_dir(dir.path()).as_deref(),
            Some("has mode 755, not 0700")
        );
        let file = dir.path().join("locks");
        std::fs::remove_dir(&file).expect("rm");
        std::fs::write(&file, b"").expect("file");
        assert_eq!(
            unsafe_template_dir(&file).as_deref(),
            Some("is not a directory")
        );
    }

    /// Store calls on a paused clock: a due timer never fires while the
    /// store waits on SQLite's threads, so no time passes at all.
    #[cfg(feature = "test-support")]
    #[tokio::test(start_paused = true)]
    async fn a_paused_clock_stands_still_while_the_store_works() {
        use crate::maintenance::StoreMaintenance;

        let started = tokio::time::Instant::now();
        let store = SqliteStore::open_temp().await;
        for _ in 0..20 {
            tokio::select! {
                biased;
                () = tokio::time::sleep(std::time::Duration::from_millis(1)) => {
                    panic!("the clock moved during a store call")
                }
                done = async {
                    store.schema_version().await.expect("read");
                    store.checkpoint_wal().await.expect("lease");
                } => done,
            }
        }
        store.close().await.expect("close");
        assert_eq!(started.elapsed(), std::time::Duration::ZERO);
    }
}
