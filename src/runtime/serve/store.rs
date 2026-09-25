//! Opening and closing the one owned store.

use std::{
    fs::DirBuilder,
    io::ErrorKind,
    os::unix::fs::DirBuilderExt,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    infrastructure::store::{SqliteStore, SqliteStoreConfig, SqliteStoreError},
    runtime::{config::StoreSettings, error::Error, logging},
};

/// Creates the lock and database directories (0700) when absent, then takes
/// ownership. Existing directories are never re-moded: the store checks them.
pub async fn open(settings: &StoreSettings) -> Result<Arc<SqliteStore>, Error> {
    create_private_dir(&settings.owner_lock_dir, "KANADE_OWNER_LOCK_DIR")?;
    if let Some(parent) = settings.db_path.parent() {
        create_private_dir(parent, "KANADE_DB_PATH")?;
    }
    let config = SqliteStoreConfig {
        db_path: settings.db_path.clone(),
        owner_lock_dir: settings.owner_lock_dir.clone(),
    };
    match SqliteStore::open(&config).await {
        Ok(store) => Ok(Arc::new(store)),
        Err(error) => Err(open_error(&config, error)),
    }
}

/// A container volume starts with only its root, and the image has no shell
/// to create these.
fn create_private_dir(path: &Path, key: &str) -> Result<(), Error> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(Error::Startup(format!(
            "{key}: its directory could not be created ({})",
            error.kind()
        ))),
    }
}

/// Store errors can carry paths and SQLite text; name the variable instead.
fn open_error(config: &SqliteStoreConfig, error: SqliteStoreError) -> Error {
    let key = |path: &Path| {
        if path.starts_with(&config.owner_lock_dir) {
            "KANADE_OWNER_LOCK_DIR"
        } else {
            "KANADE_DB_PATH"
        }
    };
    Error::Startup(match error {
        SqliteStoreError::Owned { .. } => {
            "KANADE_DB_PATH is already owned by another kanade process".into()
        }
        SqliteStoreError::UnsafePath { path, reason } => {
            format!("{}: refusing an unsafe store path ({reason})", key(&path))
        }
        SqliteStoreError::Io { path, source } => {
            format!("{}: store I/O failed ({})", key(&path), source.kind())
        }
        error @ (SqliteStoreError::ChecksumMismatch { .. }
        | SqliteStoreError::FutureVersion { .. }
        | SqliteStoreError::MigrationGap { .. }) => format!("store schema refused: {error}"),
        _ => "KANADE_DB_PATH: the store could not be opened".into(),
    })
}

/// Close after the HTTP drain. Connections the drain deadline abandoned may
/// still hold the store; wait up to `wait` for them, then leave it to exit.
pub async fn close(mut store: Arc<SqliteStore>, wait: Duration) {
    let deadline = Instant::now() + wait;
    loop {
        match Arc::try_unwrap(store) {
            Ok(owned) => {
                let closed = owned.close().await.is_ok();
                logging::store_closed(closed);
                return;
            }
            Err(shared) if Instant::now() < deadline => {
                store = shared;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(_) => {
                logging::store_closed(false);
                return;
            }
        }
    }
}
