//! Catalog, knowledge and persona locations; loading happens elsewhere.

use std::{collections::BTreeMap, path::PathBuf};

use super::non_empty;

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
            knowledge_dir: non_empty(values, "KANADE_KNOWLEDGE_DIR").map(PathBuf::from),
            persona_dir: PathBuf::from(
                non_empty(values, "KANADE_PERSONA_DIR").unwrap_or("config/personas"),
            ),
        }
    }
}
