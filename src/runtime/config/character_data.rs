//! Module `character_data` (`modules.character_data` /
//! `KANADE_MODULE_CHARACTER_DATA`, default off, needs `modules.roster`):
//! `character_data.db_path` / `KANADE_CHARACTER_DATA_DB_PATH`, the separate
//! character-data file (required while on), and the optional
//! `character_data.nexon_key_file` / `KANADE_NEXON_API_KEY_FILE` (unset:
//! the module runs with links refused; set but unreadable: startup is
//! refused, checked by serve). Off, both are ignored and no file is opened.

use std::{collections::BTreeMap, path::PathBuf};

use super::{Error, non_empty, store::absolute};

pub const FLAG_KEY: &str = "KANADE_MODULE_CHARACTER_DATA";
pub const DB_PATH_KEY: &str = "KANADE_CHARACTER_DATA_DB_PATH";
pub const NEXON_KEY_FILE_KEY: &str = "KANADE_NEXON_API_KEY_FILE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterDataSettings {
    pub db_path: PathBuf,
    pub nexon_key_file: Option<PathBuf>,
}

impl CharacterDataSettings {
    /// `None` while the module is off.
    pub(super) fn from_mapping(
        values: &BTreeMap<String, String>,
        on: bool,
        roster: bool,
    ) -> Result<Option<Self>, Error> {
        if !on {
            return Ok(None);
        }
        if !roster {
            return Err(Error::Configuration(format!(
                "{FLAG_KEY} needs the roster module (KANADE_MODULE_ROSTER=1)"
            )));
        }
        Ok(Some(Self {
            db_path: absolute(values, DB_PATH_KEY)?,
            nexon_key_file: non_empty(values, NEXON_KEY_FILE_KEY).map(PathBuf::from),
        }))
    }
}
