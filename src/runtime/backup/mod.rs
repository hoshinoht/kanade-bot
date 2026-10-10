//! `kanade backup`: a store-owning snapshot (`VACUUM INTO`) plus its
//! manifest, written into `KANADE_BACKUP_DIR` while serve is stopped. The
//! owner lock refuses it while another process owns the store. With
//! `KANADE_BACKUP_RECIPIENTS_FILE` set, the snapshot is published as
//! `<name>.age` (age, X25519) and only the manifest stays plaintext.
//! Staging dirs left by a killed earlier backup (they can hold a plaintext
//! snapshot) are removed once the lock is held.

pub mod crypt;
pub mod tools;

use std::fmt;
use std::fs::{self, File};
use std::path::Path;

use chrono::{DateTime, Datelike, Timelike, Utc};

use super::{
    config::{BACKUP_RECIPIENTS_KEY, BackupConfig},
    error::Error,
};
use crate::infrastructure::store::{
    BackupManifest, SqliteStore, SqliteStoreConfig, SqliteStoreError,
};
use crypt::Recipients;

/// Appended to the snapshot's name when it is encrypted.
pub const AGE_SUFFIX: &str = ".age";

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

/// Snapshot the store as `name` (or [`default_name`] at `now`), encrypted
/// to `config.recipients_file` as `<name>.age` when one is set.
///
/// # Errors
/// A bad recipients file (checked before the store is opened), no store at
/// the configured path (one is never created here), the store is owned
/// elsewhere, the name is taken, or the copy failed; a failed copy
/// publishes nothing.
pub async fn run(
    name: Option<String>,
    config: &BackupConfig,
    now: DateTime<Utc>,
) -> Result<Report, Error> {
    let recipients = config
        .recipients_file
        .as_deref()
        .map(|path| Recipients::load(path, BACKUP_RECIPIENTS_KEY))
        .transpose()?;
    let mut file = name.unwrap_or_else(|| default_name(now));
    if recipients.is_some() {
        file.push_str(AGE_SUFFIX);
    }
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
    remove_stale_partials(&config.dir);
    let written = match recipients {
        None => store.backup(&dest).await,
        Some(recipients) => {
            let seal = move |plain: File, sealed: File| recipients.encrypt(plain, sealed);
            store.backup_sealed(&dest, Box::new(seal)).await
        }
    };
    let closed = store.close().await;
    written.map_err(|error| store_error(&error))?;
    closed.map_err(|error| store_error(&error))?;
    let manifest = read(&dest)?;
    Ok(Report { file, manifest })
}

fn read(dest: &Path) -> Result<BackupManifest, Error> {
    BackupManifest::read(&BackupManifest::path_for(dest)).map_err(|error| store_error(&error))
}

/// Delete `.<name>.partial-<uuid>` staging dirs (the store's backup staging
/// name) in `dir`. A backup killed outright (SIGKILL, OOM, power loss) never
/// runs its cleanup, and a sealed backup's staging dir holds the plaintext
/// snapshot. Only the store owner stages there, so with the owner lock held
/// every such dir is abandoned. Best effort: the snapshot goes ahead anyway.
fn remove_stale_partials(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| is_partial(name)) else {
            continue;
        };
        // `file_type` does not follow symlinks: only real dirs are removed.
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        match fs::remove_dir_all(entry.path()) {
            Ok(()) => removed.push(name.to_owned()),
            // Error text carries the path; the name alone identifies it.
            Err(_) => crate::runtime::logging::event(
                "WARN",
                "backup_partial_not_removed",
                serde_json::json!({"dir": name}),
            ),
        }
    }
    if !removed.is_empty() {
        removed.sort();
        crate::runtime::logging::event(
            "INFO",
            "backup_partials_removed",
            serde_json::json!({"dirs": removed}),
        );
    }
}

fn is_partial(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|rest| rest.rsplit_once(".partial-"))
        .is_some_and(|(_, id)| uuid::Uuid::parse_str(id).is_ok())
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

    /// A backup killed mid-way left its staging dir (with the plaintext
    /// snapshot); the next backup, holding the lock, removes it and logs it,
    /// leaving look-alikes that are not staging dirs alone.
    #[tokio::test]
    async fn a_backup_removes_stale_staging_dirs_once_it_owns_the_store() {
        use std::os::unix::fs::DirBuilderExt;

        let root = fs::canonicalize(std::env::temp_dir()).expect("temp dir");
        let dir = root.join(format!("kanade-stale-partials-{}", uuid::Uuid::new_v4()));
        let backups = dir.join("backups");
        for path in [&dir, &dir.join("locks"), &backups] {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .expect("create");
        }
        let store = SqliteStoreConfig {
            db_path: dir.join("db.sqlite3"),
            owner_lock_dir: dir.join("locks"),
        };
        SqliteStore::open(&store)
            .await
            .expect("open")
            .close()
            .await
            .expect("close");
        let stale = format!(".old.sqlite.age.partial-{}", uuid::Uuid::new_v4());
        fs::create_dir(backups.join(&stale)).expect("stale dir");
        fs::write(backups.join(&stale).join("staged.sqlite3"), b"plain").expect("stale file");
        let not_uuid = ".old.sqlite.partial-later";
        fs::create_dir(backups.join(not_uuid)).expect("look-alike dir");
        let plain_file = format!(".old.sqlite.partial-{}", uuid::Uuid::new_v4());
        fs::write(backups.join(&plain_file), b"").expect("look-alike file");
        let config = BackupConfig {
            store: crate::runtime::config::StoreSettings {
                db_path: store.db_path.clone(),
                owner_lock_dir: store.owner_lock_dir.clone(),
            },
            dir: backups.clone(),
            recipients_file: None,
        };

        crate::runtime::logging::capture();
        let at = Utc.with_ymd_and_hms(2026, 10, 11, 3, 0, 0).unwrap();
        let report = run(Some("snap.sqlite".into()), &config, at).await;
        let lines = crate::runtime::logging::captured();
        let mut left: Vec<String> = fs::read_dir(&backups)
            .expect("list")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("utf-8")
            })
            .collect();
        left.sort();
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(report.expect("backup").file, "snap.sqlite");
        let mut expected = vec![
            not_uuid.to_owned(),
            plain_file,
            "snap.sqlite".to_owned(),
            "snap.sqlite.manifest.json".to_owned(),
        ];
        expected.sort();
        assert_eq!(left, expected);
        let removed: Vec<_> = lines
            .iter()
            .filter(|line| line["event"] == "backup_partials_removed")
            .collect();
        assert_eq!(removed.len(), 1, "{lines:?}");
        assert_eq!(removed[0]["level"], "INFO");
        assert_eq!(removed[0]["dirs"], serde_json::json!([stale]));
    }
}
