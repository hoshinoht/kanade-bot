//! Boss knowledge from the tracked `boss/knowledge/*.yaml` (schema v2). The
//! files are public, so the mock serves them as they are.

use super::{MoveError, Store};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct Knowledge {
    pub key: String,
    pub name: String,
    pub level: Option<u16>,
    pub portrait: Option<String>,
    pub hue: u16,
    pub researched_as_of: Option<String>,
    pub path: String,
    /// Difficulty letters a live weekly timing runs.
    pub in_use: Vec<String>,
    pub doc: Value,
}

#[derive(Serialize)]
pub struct EventBoss {
    pub key: String,
    pub event: Value,
    pub summary: Value,
}

fn read(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    yaml_serde::from_str(&text).ok()
}

/// A file stem from a key, refusing anything but letters and digits.
fn stem(key: &str) -> Option<String> {
    (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric())).then(|| key.to_lowercase())
}

pub struct KnowledgeDir(pub PathBuf);

impl KnowledgeDir {
    pub fn doc(&self, key: &str) -> Option<(Value, String)> {
        let stem = stem(key)?;
        let doc = read(&self.0.join(format!("{stem}.yaml")))?;
        Some((doc, format!("boss/knowledge/{stem}.yaml")))
    }

    pub fn researched_as_of(&self) -> Option<String> {
        read(&self.0.join("_meta.yaml"))?
            .get("researched_as_of")?
            .as_str()
            .map(str::to_owned)
    }

    /// Documents that declare an `event` (bosses outside the catalog, e.g. Kai).
    pub fn events(&self) -> Vec<EventBoss> {
        let Ok(entries) = std::fs::read_dir(&self.0) else {
            return Vec::new();
        };
        let mut out: Vec<EventBoss> = entries
            .flatten()
            .filter(|e| {
                e.path().extension().is_some_and(|x| x == "yaml")
                    && !e.file_name().to_string_lossy().starts_with('_')
            })
            .filter_map(|e| read(&e.path()))
            .filter_map(|doc| {
                let event = doc.get("event")?.clone();
                Some(EventBoss {
                    key: doc.get("boss")?.as_str()?.to_owned(),
                    event,
                    summary: doc.get("summary").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }
}

impl Store {
    pub fn knowledge_v2(&self, dir: &KnowledgeDir, key: &str) -> Result<Knowledge, MoveError> {
        let (doc, path) = dir.doc(key).ok_or(MoveError::NotFound)?;
        let key = doc
            .get("boss")
            .and_then(Value::as_str)
            .unwrap_or(key)
            .to_owned();
        let row = self.boss_rows().into_iter().find(|r| r.key == key);
        Ok(Knowledge {
            name: row
                .as_ref()
                .map_or_else(|| key.clone(), |r| r.name.to_owned()),
            level: row.as_ref().map(|r| r.level),
            portrait: row.as_ref().and_then(|r| r.portrait.clone()),
            hue: row.as_ref().map_or(0, |r| r.hue),
            in_use: row
                .map(|r| {
                    r.difficulties
                        .into_iter()
                        .filter(|d| d.in_use)
                        .map(|d| d.letter.to_owned())
                        .collect()
                })
                .unwrap_or_default(),
            researched_as_of: dir.researched_as_of(),
            path,
            key,
            doc,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::KnowledgeDir;
    use crate::mock::tests::store;
    use std::path::PathBuf;

    fn dir() -> KnowledgeDir {
        KnowledgeDir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../boss/knowledge"))
    }

    #[test]
    fn serves_the_tracked_schema_v2_files() {
        let k = store().knowledge_v2(&dir(), "MaleficStar").ok().unwrap();
        assert_eq!(k.name, "Radiant Malefic Star");
        assert!(
            k.doc["difficulties"]
                .as_array()
                .is_some_and(|d| !d.is_empty())
        );
        assert!(k.doc["sources"][0]["author"].is_string());
        assert_eq!(k.in_use, vec!["h".to_owned()]);
        assert!(store().knowledge_v2(&dir(), "../etc").is_err());
    }

    #[test]
    fn event_bosses_are_listed() {
        assert!(dir().events().iter().any(|e| e.key == "Kai"));
    }
}
