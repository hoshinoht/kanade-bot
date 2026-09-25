//! `bosses.json`: the catalog list, knowledge pages from the tracked
//! `boss/knowledge/*.yaml` (schema v2, public) and event bosses.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use super::{Art, hue};
use crate::domain::{catalog::BossTable, schedule::FixedRun};

const LETTERS: [&str; 5] = ["e", "n", "h", "c", "x"];
/// Knowledge files are a few KiB; anything far larger is not ours.
const MAX_KNOWLEDGE_BYTES: u64 = 256 * 1024;

#[derive(Serialize)]
pub struct DifficultyOption {
    pub letter: String,
    pub name: String,
    pub token: String,
    pub in_use: bool,
}

#[derive(Serialize)]
pub struct BossRow {
    pub key: String,
    pub name: String,
    pub level: u64,
    pub hue: u16,
    pub portrait: Option<String>,
    pub difficulties: Vec<DifficultyOption>,
}

fn in_use_tokens(fixed: &[FixedRun]) -> Vec<&str> {
    fixed
        .iter()
        .flat_map(|timing| timing.bosses.iter().map(String::as_str))
        .collect()
}

/// Every catalog boss in catalog order, with the difficulties weekly timings use.
pub fn rows(catalog: &BossTable, art: &Art<'_>, fixed: &[FixedRun]) -> Vec<BossRow> {
    let in_use = in_use_tokens(fixed);
    catalog
        .ordered()
        .into_iter()
        .map(|boss| {
            let key = boss.short();
            BossRow {
                key: key.to_owned(),
                name: boss.full().to_owned(),
                level: boss.level().unwrap_or(0),
                hue: hue(boss.guide_colour()),
                portrait: art.url("portraits", key, boss.portrait().unwrap_or(key)),
                difficulties: boss
                    .difficulties()
                    .iter()
                    .filter(|letter| LETTERS.contains(&letter.as_str()))
                    .map(|letter| {
                        let token = boss.canonical(letter);
                        DifficultyOption {
                            letter: letter.clone(),
                            name: catalog.difficulty_name(letter),
                            in_use: in_use.contains(&token.as_str()),
                            token,
                        }
                    })
                    .collect(),
            }
        })
        .collect()
}

fn read_yaml(path: &Path) -> Option<Value> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_KNOWLEDGE_BYTES {
        return None;
    }
    serde_saphyr::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The file stem for a key: ASCII letters and digits only, lowercased.
fn stem(key: &str) -> Option<String> {
    (!key.is_empty() && key.len() <= 64 && key.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .then(|| key.to_ascii_lowercase())
}

#[derive(Serialize)]
pub struct Knowledge {
    pub key: String,
    pub name: String,
    pub level: Option<u64>,
    pub portrait: Option<String>,
    pub hue: u16,
    pub researched_as_of: Option<String>,
    pub path: String,
    pub in_use: Vec<String>,
    pub doc: Value,
}

pub fn knowledge(
    dir: &Path,
    catalog: &BossTable,
    art: &Art<'_>,
    fixed: &[FixedRun],
    key: &str,
) -> Option<Knowledge> {
    let stem = stem(key)?;
    let doc = read_yaml(&dir.join(format!("{stem}.yaml")))?;
    let key = doc.get("boss").and_then(Value::as_str)?.to_owned();
    let boss = catalog.boss(&key);
    let in_use = in_use_tokens(fixed);
    Some(Knowledge {
        name: boss.map_or_else(|| key.clone(), |boss| boss.full().to_owned()),
        level: boss.and_then(|boss| boss.level()),
        portrait: boss.and_then(|boss| art.url("portraits", &key, boss.portrait().unwrap_or(&key))),
        hue: boss.map_or(0, |boss| hue(boss.guide_colour())),
        researched_as_of: read_yaml(&dir.join("_meta.yaml"))
            .and_then(|meta| meta.get("researched_as_of")?.as_str().map(str::to_owned)),
        path: format!("boss/knowledge/{stem}.yaml"),
        in_use: boss
            .map(|boss| {
                boss.difficulties()
                    .iter()
                    .filter(|letter| in_use.contains(&boss.canonical(letter).as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
        key,
        doc,
    })
}

#[derive(Serialize)]
pub struct EventBoss {
    pub key: String,
    pub event: Value,
    pub summary: Value,
}

/// Documents that declare an `event` (bosses outside the catalog, e.g. Kai).
pub fn events(dir: &Path) -> Vec<EventBoss> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<EventBoss> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yaml")
                && path
                    .file_name()
                    .is_some_and(|name| !name.to_string_lossy().starts_with('_'))
        })
        .filter_map(|path| read_yaml(&path))
        .filter_map(|doc| {
            Some(EventBoss {
                key: doc.get("boss")?.as_str()?.to_owned(),
                event: doc.get("event")?.clone(),
                summary: doc
                    .get("summary")
                    .cloned()
                    .unwrap_or(Value::String(String::new())),
            })
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}
