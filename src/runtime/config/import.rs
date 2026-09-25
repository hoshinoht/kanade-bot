//! `import v4` configuration: only what the import touches (no Discord,
//! listeners or secrets).

use std::collections::BTreeMap;

use chrono_tz::Tz;

use super::{Error, FileSettings, StoreSettings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportConfig {
    pub timezone: Tz,
    pub store: StoreSettings,
    pub files: FileSettings,
}

impl ImportConfig {
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        let timezone = super::non_empty(values, "KANADE_TIMEZONE")
            .ok_or_else(|| Error::Configuration("KANADE_TIMEZONE is required".into()))?
            .parse::<Tz>()
            .map_err(|_| {
                Error::Configuration("KANADE_TIMEZONE must be a valid IANA timezone".into())
            })?;
        Ok(Self {
            timezone,
            store: StoreSettings::from_mapping(values)?,
            files: FileSettings::from_mapping(values),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_timezone_and_store_paths_only() {
        let mut values = BTreeMap::from([
            ("KANADE_TIMEZONE".to_owned(), "Asia/Kuala_Lumpur".to_owned()),
            (
                "KANADE_DB_PATH".to_owned(),
                "/data/kanade.sqlite".to_owned(),
            ),
        ]);
        assert_eq!(
            ImportConfig::from_mapping(&values).unwrap_err().to_string(),
            "KANADE_OWNER_LOCK_DIR is required"
        );
        values.insert("KANADE_OWNER_LOCK_DIR".into(), "/data/run".into());
        let config = ImportConfig::from_mapping(&values).unwrap();
        assert_eq!(config.timezone, chrono_tz::Asia::Kuala_Lumpur);
        assert_eq!(
            config.files.catalog_file,
            std::path::PathBuf::from("boss/bosses.yaml")
        );
    }
}
