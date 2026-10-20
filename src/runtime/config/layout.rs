//! Paths inside the image: set by `deploy/Dockerfile`'s `ENV`, never by
//! `kanade.toml` (local runs and tests set the variables). Only the shape is
//! checked here; ownership, symlink and permission checks run where each is
//! opened.

use std::{
    collections::BTreeMap,
    path::{Component, PathBuf},
};

use super::{Error, non_empty};

pub const DB_PATH: &str = "KANADE_DB_PATH";
pub const OWNER_LOCK_DIR: &str = "KANADE_OWNER_LOCK_DIR";
pub const BACKUP_DIR: &str = "KANADE_BACKUP_DIR";
pub const CHARACTER_DATA_DB_PATH: &str = "KANADE_CHARACTER_DATA_DB_PATH";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreSettings {
    pub db_path: PathBuf,
    pub owner_lock_dir: PathBuf,
}

impl StoreSettings {
    /// `KANADE_DB_PATH` and `KANADE_OWNER_LOCK_DIR` alone (`kanade restore`).
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        Ok(Self {
            db_path: absolute(values, DB_PATH)?,
            owner_lock_dir: absolute(values, OWNER_LOCK_DIR)?,
        })
    }
}

/// Catalog, knowledge and persona locations; loading happens elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSettings {
    pub catalog_file: PathBuf,
    pub knowledge_dir: Option<PathBuf>,
    pub persona_dir: PathBuf,
}

impl FileSettings {
    pub(super) fn from_mapping(values: &BTreeMap<String, String>) -> Self {
        Self {
            catalog_file: PathBuf::from(
                non_empty(values, "KANADE_CATALOG_FILE").unwrap_or("boss/bosses.yaml"),
            ),
            knowledge_dir: optional(values, "KANADE_KNOWLEDGE_DIR"),
            persona_dir: PathBuf::from(
                non_empty(values, "KANADE_PERSONA_DIR").unwrap_or("config/personas"),
            ),
        }
    }
}

/// The web workspace root (`apps/{admin,public}/dist`), boss art and the
/// bot's identity cache, in that order.
pub(super) fn web_dirs(
    values: &BTreeMap<String, String>,
) -> (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) {
    (
        optional(values, "KANADE_WEB_DIR"),
        optional(values, "KANADE_BOSS_DIR"),
        optional(values, "KANADE_IDENTITY_DIR"),
    )
}

fn optional(values: &BTreeMap<String, String>, key: &str) -> Option<PathBuf> {
    non_empty(values, key).map(PathBuf::from)
}

/// Required, absolute and without `..`.
pub(super) fn absolute(values: &BTreeMap<String, String>, key: &str) -> Result<PathBuf, Error> {
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
