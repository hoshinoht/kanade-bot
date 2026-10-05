//! `bosses.json`: the catalog list, knowledge pages from the tracked
//! `boss/knowledge/*.yaml` (schema v2, public) and event bosses.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use super::{Art, hue};
use crate::{
    domain::{catalog::BossTable, schedule::FixedRun},
    infrastructure::files::read_document,
};

const LETTERS: [&str; 5] = ["e", "n", "h", "c", "x"];

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DifficultyOption {
    #[cfg_attr(test, ts(type = "Difficulty"))]
    pub letter: String,
    pub name: String,
    pub token: String,
    pub in_use: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
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
    read_document(path).ok()
}

/// The file stem for a key: ASCII letters and digits only, lowercased.
fn stem(key: &str) -> Option<String> {
    (!key.is_empty() && key.len() <= 64 && key.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .then(|| key.to_ascii_lowercase())
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Knowledge {
    pub key: String,
    pub name: String,
    pub level: Option<u64>,
    pub portrait: Option<String>,
    /// The looping MP4 (`/art/animated/{key}`); null where the deployment has none.
    pub animated: Option<String>,
    pub hue: u16,
    pub researched_as_of: Option<String>,
    pub path: String,
    #[cfg_attr(test, ts(type = "Difficulty[]"))]
    pub in_use: Vec<String>,
    #[cfg_attr(test, ts(type = "KnowledgeDoc"))]
    pub doc: Value,
    /// Every boss in the series of a mission this doc defines, sorted by
    /// series, `order` and key; empty when the doc has no mission.
    pub missions: Vec<MissionStop>,
}

/// One boss's place in a mission series (Destiny Weapon, Union Champion).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MissionStop {
    #[cfg_attr(test, ts(type = "MissionSeries"))]
    pub series: String,
    pub order: u8,
    pub key: String,
    pub name: String,
    #[cfg_attr(test, ts(type = "DifficultyName"))]
    pub difficulty: String,
}

const SERIES: [&str; 2] = ["destiny-weapon", "union-champion"];
const DIFFICULTY_NAMES: [&str; 7] = [
    "Easy", "Normal", "Hard", "Chaos", "Extreme", "Champion", "Destiny",
];

/// `(series, order, difficulty name)` of each well-formed mission in `doc`;
/// anything outside the contract's vocabulary is skipped, never an error.
fn doc_missions(doc: &Value) -> Vec<(&str, u8, &str)> {
    doc.get("difficulties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|facts| {
            let mission = facts.get("mission")?;
            let series = mission.get("series")?.as_str()?;
            let order = u8::try_from(mission.get("order")?.as_u64()?).ok()?;
            let difficulty = facts.get("name")?.as_str()?;
            (SERIES.contains(&series) && DIFFICULTY_NAMES.contains(&difficulty))
                .then_some((series, order, difficulty))
        })
        .collect()
}

/// Every boss in `dir` with a mission in a series `doc` has one in, sorted
/// by series, `order` and key. Sibling documents are re-read per request;
/// an unreadable one, or one not at its lowercased `boss` stem, is skipped.
fn mission_stops(dir: &Path, catalog: &BossTable, doc: &Value) -> Vec<MissionStop> {
    let wanted: Vec<&str> = doc_missions(doc)
        .into_iter()
        .map(|(series, ..)| series)
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut stops: Vec<MissionStop> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name
                .strip_suffix(".yaml")
                .filter(|_| !name.starts_with('_'))?;
            let sibling = read_yaml(&entry.path())?;
            let key = sibling.get("boss")?.as_str()?;
            (key.to_ascii_lowercase() == stem).then_some(())?;
            let name = catalog.boss(key).map_or(key, |boss| boss.full());
            Some(
                doc_missions(&sibling)
                    .into_iter()
                    .filter(|(series, ..)| wanted.contains(series))
                    .map(|(series, order, difficulty)| MissionStop {
                        series: series.to_owned(),
                        order,
                        key: key.to_owned(),
                        name: name.to_owned(),
                        difficulty: difficulty.to_owned(),
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect();
    stops.sort_by(|a, b| (&a.series, a.order, &a.key).cmp(&(&b.series, b.order, &b.key)));
    stops
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
    // Event bosses' art is named by their key (`/art` resolves them the same way).
    let basename = match boss {
        Some(boss) => Some(boss.portrait().unwrap_or(&key)),
        None if doc.get("event").is_some() => Some(key.as_str()),
        None => None,
    };
    let url = |kind| basename.and_then(|basename| art.url(kind, &key, basename));
    let (portrait, animated) = (url("portraits"), url("animated"));
    Some(Knowledge {
        name: boss.map_or_else(|| key.clone(), |boss| boss.full().to_owned()),
        level: boss.and_then(|boss| boss.level()),
        portrait,
        animated,
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
        missions: mission_stops(dir, catalog, &doc),
        key,
        doc,
    })
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EventBoss {
    pub key: String,
    #[cfg_attr(test, ts(type = "{ name: string; availability: string }"))]
    pub event: Value,
    #[cfg_attr(test, ts(type = "string"))]
    pub summary: Value,
    pub portrait: Option<String>,
    pub portrait_sm: Option<String>,
    pub art: Option<String>,
    pub animated: Option<String>,
}

/// Whether an event document declares exactly `key` (case-sensitive), so
/// `/art` may serve art under that basename.
pub fn is_event(dir: &Path, key: &str) -> bool {
    stem(key)
        .and_then(|stem| read_yaml(&dir.join(format!("{stem}.yaml"))))
        .is_some_and(|doc| {
            doc.get("event").is_some() && doc.get("boss").and_then(Value::as_str) == Some(key)
        })
}

/// Documents that declare an `event` (bosses outside the catalog, e.g. Kai).
pub fn events(dir: &Path, art: &Art<'_>) -> Vec<EventBoss> {
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
            let key = doc.get("boss")?.as_str()?.to_owned();
            let portrait = art.url("portraits", &key, &key);
            Some(EventBoss {
                event: doc.get("event")?.clone(),
                summary: doc
                    .get("summary")
                    .cloned()
                    .unwrap_or(Value::String(String::new())),
                portrait_sm: art.url("icons", &key, &key).or_else(|| portrait.clone()),
                portrait,
                art: art.url("entry", &key, &key),
                animated: art.url("animated", &key, &key),
                key,
            })
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}
