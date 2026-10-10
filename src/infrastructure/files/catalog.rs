//! `boss/bosses.yaml` → [`BossTable`].

use std::{fmt, marker::PhantomData, path::Path};

use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_saphyr::{DuplicateKeyPolicy, MergeKeyPolicy};

use super::{LoadError, read::read_text};
use crate::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec, GuideSpec};

const MAX_CATALOG_BYTES: u64 = 256 * 1024;

/// A mapping kept in file order; boss order is display order.
struct Ordered<T>(Vec<(String, T)>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Ordered<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OrderedVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for OrderedVisitor<T> {
            type Value = Ordered<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a mapping")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, T>()? {
                    entries.push(entry);
                }
                Ok(Ordered(entries))
            }
        }

        deserializer.deserialize_map(OrderedVisitor(PhantomData))
    }
}

/// Optional keys may be absent but never explicitly null.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalog {
    difficulties: Ordered<String>,
    bosses: Ordered<RawBoss>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBoss {
    #[serde(default, deserialize_with = "present")]
    full: Option<String>,
    #[serde(default, deserialize_with = "present")]
    level: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    difficulties: Option<Vec<String>>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default, deserialize_with = "present")]
    portrait: Option<String>,
    #[serde(default, deserialize_with = "present")]
    guide: Option<RawGuide>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuide {
    #[serde(default, deserialize_with = "present")]
    colour: Option<i64>,
}

fn spec(raw: RawCatalog) -> CatalogSpec {
    CatalogSpec {
        difficulties: raw
            .difficulties
            .0
            .into_iter()
            .map(|(prefix, label)| DifficultySpec { prefix, label })
            .collect(),
        bosses: raw
            .bosses
            .0
            .into_iter()
            .map(|(short, boss)| BossSpec {
                short,
                full: boss.full,
                level: boss.level,
                difficulties: boss.difficulties,
                aliases: boss.aliases,
                portrait: boss.portrait,
                guide: boss.guide.map(|guide| GuideSpec {
                    colour: guide.colour,
                }),
            })
            .collect(),
    }
}

/// Strictly parses and validates the boss catalog.
pub fn load_catalog(path: &Path) -> Result<BossTable, LoadError> {
    let text = read_text(path, MAX_CATALOG_BYTES)?;
    let options = serde_saphyr::options! {
        budget: serde_saphyr::budget! {
            max_documents: 1,
            max_depth: 8,
            max_anchors: 0,
            max_aliases: 0,
            max_merge_keys: 0,
        },
        duplicate_keys: DuplicateKeyPolicy::Error,
        merge_keys: MergeKeyPolicy::Error,
        reject_unsupported_tags: true,
        strict_booleans: true,
        with_snippet: false,
    };
    let raw: RawCatalog = serde_saphyr::from_str_with_options(&text, options)
        .map_err(|error| LoadError::new(path, format!("invalid catalog YAML: {error}")))?;
    BossTable::from_spec(&spec(raw)).map_err(|error| LoadError::new(path, error.message()))
}
