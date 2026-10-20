//! `KANADE_BACKUP_DIR` (set by the image): where `kanade backup` writes
//! snapshots and their manifests, and where serve lists them (read only) for
//! History. Its optional `recipients` file holds age public keys; while it
//! is present, snapshots are written encrypted (`<name>.age`).

use std::{collections::BTreeMap, path::PathBuf};

use super::{
    Error, StoreSettings,
    layout::{BACKUP_DIR, absolute},
    non_empty,
};

/// The age recipients' fixed name in the backup directory.
const RECIPIENTS: &str = "recipients";
/// How errors name the recipients file.
pub const RECIPIENTS_LABEL: &str = "backups file `recipients`";

/// `kanade backup`: the store paths and the backup directory only (no
/// timezone, Discord, listeners or secrets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupConfig {
    pub store: StoreSettings,
    pub dir: PathBuf,
    /// `<dir>/recipients`; absent writes plaintext snapshots.
    pub recipients_file: PathBuf,
}

impl BackupConfig {
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        let dir = absolute(values, BACKUP_DIR)?;
        Ok(Self {
            store: StoreSettings::from_mapping(values)?,
            recipients_file: dir.join(RECIPIENTS),
            dir,
        })
    }
}

/// `<KANADE_BACKUP_DIR>/recipients` when the directory is set; whether the
/// file is there is the caller's to check.
pub fn recipients_file(values: &BTreeMap<String, String>) -> Option<PathBuf> {
    non_empty(values, BACKUP_DIR).map(|dir| PathBuf::from(dir).join(RECIPIENTS))
}

/// Optional for serve: unset lists no backups.
pub(super) fn optional_dir(values: &BTreeMap<String, String>) -> Result<Option<PathBuf>, Error> {
    non_empty(values, BACKUP_DIR)
        .map(|_| absolute(values, BACKUP_DIR))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn needs_store_paths_and_an_absolute_dir() {
        let store = [
            ("KANADE_DB_PATH", "/data/db/kanade.sqlite"),
            ("KANADE_OWNER_LOCK_DIR", "/data/run"),
        ];
        assert_eq!(
            BackupConfig::from_mapping(&values(&store))
                .unwrap_err()
                .to_string(),
            "KANADE_BACKUP_DIR is required"
        );
        let mut pairs = store.to_vec();
        pairs.push((BACKUP_DIR, "backups"));
        assert_eq!(
            BackupConfig::from_mapping(&values(&pairs))
                .unwrap_err()
                .to_string(),
            "KANADE_BACKUP_DIR must be an absolute path without `..`"
        );
        pairs.pop();
        pairs.push((BACKUP_DIR, "/backups"));
        let config = BackupConfig::from_mapping(&values(&pairs)).unwrap();
        assert_eq!(config.dir, PathBuf::from("/backups"));
        assert_eq!(config.store.owner_lock_dir, PathBuf::from("/data/run"));
        assert_eq!(config.recipients_file, PathBuf::from("/backups/recipients"));
        assert_eq!(
            recipients_file(&values(&pairs)),
            Some(PathBuf::from("/backups/recipients"))
        );
        assert_eq!(recipients_file(&values(&store)), None);
    }

    #[test]
    fn serve_treats_the_dir_as_optional() {
        assert_eq!(optional_dir(&values(&[])).unwrap(), None);
        assert_eq!(optional_dir(&values(&[(BACKUP_DIR, "")])).unwrap(), None);
        assert_eq!(
            optional_dir(&values(&[(BACKUP_DIR, "/backups")])).unwrap(),
            Some(PathBuf::from("/backups"))
        );
        assert!(optional_dir(&values(&[(BACKUP_DIR, "/a/../b")])).is_err());
    }
}
