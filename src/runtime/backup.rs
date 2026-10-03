//! `kanade backup`: a store-owning snapshot (`VACUUM INTO`) plus its
//! manifest, written into `KANADE_BACKUP_DIR` while serve is stopped. The
//! owner lock refuses it while another process owns the store.

use std::fmt;
use std::path::Path;

use chrono::{DateTime, Datelike, Timelike, Utc};

use super::{config::BackupConfig, error::Error};
use crate::infrastructure::store::{
    BackupManifest, SqliteStore, SqliteStoreConfig, SqliteStoreError,
};

/// What was written, printed for the operator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub file: String,
    pub manifest: BackupManifest,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "backup {}: history head {} ({}), revision {}, schema {}",
            self.file,
            self.manifest.history_head.seq,
            self.manifest.history_head.hash,
            self.manifest.revision,
            self.manifest.schema_version
        )
    }
}

/// `kanade-20261003T070900Z.sqlite`.
pub fn default_name(now: DateTime<Utc>) -> String {
    format!(
        "kanade-{:04}{:02}{:02}T{:02}{:02}{:02}Z.sqlite",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn store_error(error: &SqliteStoreError) -> Error {
    match error {
        SqliteStoreError::Owned { .. } => Error::Unavailable(
            "backup: the store is owned by another process; stop the bot first".into(),
        ),
        SqliteStoreError::Exists { .. } => {
            Error::Configuration(format!("backup: {error}; choose another --name"))
        }
        _ => Error::Unavailable(format!("backup: {error}")),
    }
}

/// Snapshot the store as `name` (or [`default_name`] at `now`).
///
/// # Errors
/// No store at the configured path (one is never created here), the store
/// is owned elsewhere, the name is taken, or the copy failed; a failed copy
/// publishes nothing.
pub async fn run(
    name: Option<String>,
    config: &BackupConfig,
    now: DateTime<Utc>,
) -> Result<Report, Error> {
    let file = name.unwrap_or_else(|| default_name(now));
    let db_path = &config.store.db_path;
    // `open` would create an empty store; a backup of nothing is a mistake.
    if !db_path.is_file() {
        return Err(Error::Configuration(
            "backup: no store at KANADE_DB_PATH".into(),
        ));
    }
    if !config.dir.is_dir() {
        return Err(Error::Configuration(
            "backup: KANADE_BACKUP_DIR is not a directory".into(),
        ));
    }
    let dest = config.dir.join(&file);
    let store = SqliteStore::open(&SqliteStoreConfig {
        db_path: db_path.clone(),
        owner_lock_dir: config.store.owner_lock_dir.clone(),
    })
    .await
    .map_err(|error| store_error(&error))?;
    let written = store.backup(&dest).await;
    let closed = store.close().await;
    written.map_err(|error| store_error(&error))?;
    closed.map_err(|error| store_error(&error))?;
    let manifest = read(&dest)?;
    Ok(Report { file, manifest })
}

fn read(dest: &Path) -> Result<BackupManifest, Error> {
    BackupManifest::read(&BackupManifest::path_for(dest)).map_err(|error| store_error(&error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn default_names_sort_by_time() {
        let at = Utc.with_ymd_and_hms(2026, 10, 3, 7, 9, 5).unwrap();
        assert_eq!(default_name(at), "kanade-20261003T070905Z.sqlite");
    }
}
