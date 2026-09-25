//! SQLite store paths. Only the shape is checked here; ownership, symlink and
//! permission checks run when the store opens.

use std::{
    collections::BTreeMap,
    path::{Component, PathBuf},
};

use super::{Error, non_empty};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreSettings {
    pub db_path: PathBuf,
    pub owner_lock_dir: PathBuf,
}

impl StoreSettings {
    pub(super) fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        Ok(Self {
            db_path: absolute(values, "KANADE_DB_PATH")?,
            owner_lock_dir: absolute(values, "KANADE_OWNER_LOCK_DIR")?,
        })
    }
}

fn absolute(values: &BTreeMap<String, String>, key: &str) -> Result<PathBuf, Error> {
    let path = PathBuf::from(
        non_empty(values, key).ok_or_else(|| Error::Configuration(format!("{key} is required")))?,
    );
    let plain = path
        .components()
        .all(|component| matches!(component, Component::RootDir | Component::Normal(_)));
    if !path.is_absolute() || !plain {
        return Err(Error::Configuration(format!(
            "{key} must be an absolute path without `..`"
        )));
    }
    Ok(path)
}
