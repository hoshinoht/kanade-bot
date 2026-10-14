//! `KANADE_BACKUP_DIR`: where `kanade backup` writes snapshots and their
//! manifests, and where serve lists them (read only) for History.
//! `KANADE_BACKUP_RECIPIENTS_FILE` (`backup.recipients_file`): optional age
//! public keys; when set, snapshots are written encrypted (`<name>.age`).

use std::{collections::BTreeMap, path::PathBuf};

use super::{Error, StoreSettings, non_empty, store::absolute};

const KEY: &str = "KANADE_BACKUP_DIR";
pub const RECIPIENTS_KEY: &str = "KANADE_BACKUP_RECIPIENTS_FILE";

/// `kanade backup`: the store paths, the backup directory and the optional
/// recipients file only (no timezone, Discord, listeners or secrets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupConfig {
    pub store: StoreSettings,
    pub dir: PathBuf,
    /// [`RECIPIENTS_KEY`]; `None` writes plaintext snapshots.
    pub recipients_file: Option<PathBuf>,
}

impl BackupConfig {
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        Ok(Self {
            store: StoreSettings::from_mapping(values)?,
            dir: absolute(values, KEY)?,
            recipients_file: recipients_file(values),
        })
    }
}

/// [`RECIPIENTS_KEY`] when set; serve only validates the file at startup.
pub fn recipients_file(values: &BTreeMap<String, String>) -> Option<PathBuf> {
    non_empty(values, RECIPIENTS_KEY).map(PathBuf::from)
}

/// Optional for serve: unset lists no backups.
pub(super) fn optional_dir(values: &BTreeMap<String, String>) -> Result<Option<PathBuf>, Error> {
    non_empty(values, KEY)
        .map(|_| absolute(values, KEY))
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
        pairs.push((KEY, "backups"));
        assert_eq!(
            BackupConfig::from_mapping(&values(&pairs))
                .unwrap_err()
                .to_string(),
            "KANADE_BACKUP_DIR must be an absolute path without `..`"
        );
        pairs.pop();
        pairs.push((KEY, "/backups"));
        let config = BackupConfig::from_mapping(&values(&pairs)).unwrap();
        assert_eq!(config.dir, PathBuf::from("/backups"));
        assert_eq!(config.store.owner_lock_dir, PathBuf::from("/data/run"));
        assert_eq!(config.recipients_file, None);
        pairs.push((RECIPIENTS_KEY, "/run/secrets/backup_recipients"));
        assert_eq!(
            BackupConfig::from_mapping(&values(&pairs))
                .unwrap()
                .recipients_file,
            Some(PathBuf::from("/run/secrets/backup_recipients"))
        );
    }

    #[test]
    fn serve_treats_the_dir_as_optional() {
        assert_eq!(optional_dir(&values(&[])).unwrap(), None);
        assert_eq!(optional_dir(&values(&[(KEY, "")])).unwrap(), None);
        assert_eq!(
            optional_dir(&values(&[(KEY, "/backups")])).unwrap(),
            Some(PathBuf::from("/backups"))
        );
        assert!(optional_dir(&values(&[(KEY, "/a/../b")])).is_err());
    }
}
