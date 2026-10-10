//! Boss knowledge from the tracked `boss/knowledge/*.yaml` (schema v2). The
//! files are public, so the mock serves them as they are.

use super::{
    MoveError, Store,
    catalog::{Catalog, Kind},
};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct Knowledge {
    pub key: String,
    pub name: String,
    pub level: Option<u16>,
    pub portrait: Option<String>,
    /// The looping MP4 (`/art/animated/{key}`); null where the deployment has none.
    pub animated: Option<String>,
    pub hue: u16,
    pub researched_as_of: Option<String>,
    pub path: String,
    /// Difficulty letters a live weekly timing runs.
    pub in_use: Vec<String>,
    pub doc: Value,
    /// As the server: every boss in the series of this doc's missions.
    pub missions: Vec<Value>,
}

/// `public.json` `PublicKnowledge`: [`Knowledge`] without `path`, and with
/// every bullet's chatbot-only `detail` removed, as the server answers members.
#[derive(Serialize)]
pub struct PublicKnowledge {
    pub key: String,
    pub name: String,
    pub level: Option<u16>,
    pub portrait: Option<String>,
    pub animated: Option<String>,
    pub hue: u16,
    pub researched_as_of: Option<String>,
    pub in_use: Vec<String>,
    pub doc: Value,
    pub missions: Vec<Value>,
}

impl From<Knowledge> for PublicKnowledge {
    fn from(knowledge: Knowledge) -> Self {
        let mut doc = knowledge.doc;
        strip_detail(&mut doc);
        Self {
            key: knowledge.key,
            name: knowledge.name,
            level: knowledge.level,
            portrait: knowledge.portrait,
            animated: knowledge.animated,
            hue: knowledge.hue,
            researched_as_of: knowledge.researched_as_of,
            in_use: knowledge.in_use,
            doc,
            missions: knowledge.missions,
        }
    }
}

fn strip_detail(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("detail");
            map.values_mut().for_each(strip_detail);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_detail),
        _ => {}
    }
}

#[derive(Serialize)]
pub struct EventBoss {
    pub key: String,
    pub event: Value,
    pub summary: Value,
    pub portrait: Option<String>,
    pub portrait_sm: Option<String>,
    pub art: Option<String>,
    pub animated: Option<String>,
}

fn read(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    yaml_serde::from_str(&text).ok()
}

/// A file stem from a key, refusing anything but letters and digits.
fn stem(key: &str) -> Option<String> {
    (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric())).then(|| key.to_lowercase())
}

const SERIES: [&str; 2] = ["destiny-weapon", "union-champion"];
const DIFFICULTY_NAMES: [&str; 7] = [
    "Easy", "Normal", "Hard", "Chaos", "Extreme", "Champion", "Destiny",
];

/// `(series, order, difficulty name)` of each well-formed mission in `doc`.
fn missions(doc: &Value) -> Vec<(&str, u64, &str)> {
    doc.get("difficulties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|facts| {
            let mission = facts.get("mission")?;
            let series = mission.get("series")?.as_str()?;
            let order = mission.get("order")?.as_u64().filter(|o| *o <= 255)?;
            let difficulty = facts.get("name")?.as_str()?;
            (SERIES.contains(&series) && DIFFICULTY_NAMES.contains(&difficulty))
                .then_some((series, order, difficulty))
        })
        .collect()
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

    /// Whether an event document declares exactly `key` (case-sensitive).
    pub fn is_event(&self, key: &str) -> bool {
        self.doc(key).is_some_and(|(doc, _)| {
            doc.get("event").is_some() && doc.get("boss").and_then(Value::as_str) == Some(key)
        })
    }

    /// As the server: every boss with a mission in a series `doc` has one in,
    /// by series, `order` and key; unreadable or misnamed siblings are skipped.
    pub fn mission_stops(&self, doc: &Value, name: impl Fn(&str) -> String) -> Vec<Value> {
        let wanted: Vec<&str> = missions(doc).into_iter().map(|(s, ..)| s).collect();
        if wanted.is_empty() {
            return Vec::new();
        }
        let Ok(entries) = std::fs::read_dir(&self.0) else {
            return Vec::new();
        };
        let mut stops: Vec<(String, u64, String, String, String)> = Vec::new();
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = file
                .strip_suffix(".yaml")
                .filter(|_| !file.starts_with('_'))
            else {
                continue;
            };
            let Some(sibling) = read(&entry.path()) else {
                continue;
            };
            let Some(key) = sibling.get("boss").and_then(Value::as_str) else {
                continue;
            };
            if key.to_lowercase() != stem {
                continue;
            }
            for (series, order, difficulty) in missions(&sibling) {
                if wanted.contains(&series) {
                    stops.push((
                        series.to_owned(),
                        order,
                        key.to_owned(),
                        name(key),
                        difficulty.to_owned(),
                    ));
                }
            }
        }
        stops.sort_by(|a, b| (&a.0, a.1, &a.2).cmp(&(&b.0, b.1, &b.2)));
        stops
            .into_iter()
            .map(|(series, order, key, name, difficulty)| {
                serde_json::json!({
                    "series": series, "order": order, "key": key,
                    "name": name, "difficulty": difficulty,
                })
            })
            .collect()
    }

    /// Documents that declare an `event` (bosses outside the catalog, e.g. Kai).
    pub fn events(&self, catalog: &Catalog) -> Vec<EventBoss> {
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
                let key = doc.get("boss")?.as_str()?.to_owned();
                let (portrait, portrait_sm, art) = catalog.event_art(&key);
                Some(EventBoss {
                    animated: catalog.event_url(Kind::Animated, &key),
                    key,
                    event,
                    summary: doc.get("summary").cloned().unwrap_or(Value::Null),
                    portrait,
                    portrait_sm,
                    art,
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
        let rows = self.boss_rows();
        let missions = dir.mission_stops(&doc, |key| {
            rows.iter()
                .find(|r| r.key == key)
                .map_or_else(|| key.to_owned(), |r| r.name.to_owned())
        });
        let row = rows.into_iter().find(|r| r.key == key);
        let (portrait, animated) = match &row {
            Some(r) => (r.portrait.clone(), self.catalog().url(Kind::Animated, &key)),
            None if doc.get("event").is_some() => (
                self.catalog().event_url(Kind::Portrait, &key),
                self.catalog().event_url(Kind::Animated, &key),
            ),
            None => (None, None),
        };
        Ok(Knowledge {
            name: row
                .as_ref()
                .map_or_else(|| key.clone(), |r| r.name.to_owned()),
            level: row.as_ref().map(|r| r.level),
            portrait,
            animated,
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
            missions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::KnowledgeDir;
    use crate::mock::{
        Store,
        catalog::{Catalog, Kind},
        tests::store,
    };
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
        let catalog = Catalog::new(PathBuf::from("/nonexistent"));
        let events = dir().events(&catalog);
        let kai = events.iter().find(|e| e.key == "Kai").unwrap();
        // No art in this deployment: null, never a broken URL.
        assert_eq!(
            (&kai.portrait, &kai.portrait_sm, &kai.art, &kai.animated),
            (&None, &None, &None, &None)
        );
    }

    /// An invented, mixed-case event boss with art (Linux CI is case-sensitive).
    struct EventFixture(PathBuf);

    impl EventFixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("pwa-mock-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for (path, body) in [
                (
                    "knowledge/starwyrm.yaml",
                    "boss: StarWyrm\nevent:\n  name: Wyrmfall Trials Season 9\n  availability: Invented.\nsummary: Invented.\n",
                ),
                (
                    "knowledge/plainwyrm.yaml",
                    "boss: PlainWyrm\nsummary: No event.\n",
                ),
                ("boss/portraits/StarWyrm.png", "art"),
                ("boss/portraits/icon/StarWyrm.png", "art"),
                ("boss/artwork/entry/StarWyrm.png", "art"),
                ("boss/artwork/animated/StarWyrm.mp4", "art"),
                ("boss/portraits/ReelWyrm.mp4", "art"),
                ("boss/artwork/animated/StillWyrm.png", "art"),
                ("boss/portraits/PlainWyrm.png", "art"),
            ] {
                let path = root.join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, body).unwrap();
            }
            Self(root)
        }
    }

    impl Drop for EventFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn event_bosses_carry_their_art_by_exact_key() {
        let fixture = EventFixture::new("event-art");
        let dir = KnowledgeDir(fixture.0.join("knowledge"));
        let catalog = Catalog::new(fixture.0.join("boss"));
        let events = dir.events(&catalog);
        assert_eq!(events.len(), 1);
        let wyrm = &events[0];
        assert_eq!(wyrm.key, "StarWyrm");
        assert_eq!(wyrm.portrait.as_deref(), Some("/art/portraits/StarWyrm"));
        assert_eq!(wyrm.portrait_sm.as_deref(), Some("/art/icons/StarWyrm"));
        assert_eq!(wyrm.art.as_deref(), Some("/art/entry/StarWyrm"));
        assert_eq!(wyrm.animated.as_deref(), Some("/art/animated/StarWyrm"));
        // Still kinds never serve video, and `animated` never serves stills.
        assert!(catalog.event_file(Kind::Portrait, "ReelWyrm").is_none());
        assert!(catalog.event_file(Kind::Animated, "StillWyrm").is_none());

        assert!(dir.is_event("StarWyrm"));
        for key in ["starwyrm", "STARWYRM", "MoonWyrm", "PlainWyrm", "../etc"] {
            assert!(!dir.is_event(key), "{key}");
        }
        assert!(catalog.event_file(Kind::Icon, "StarWyrm").is_some());
        assert!(catalog.event_file(Kind::Portrait, "../StarWyrm").is_none());

        let k = Store::new(catalog)
            .knowledge_v2(&dir, "StarWyrm")
            .ok()
            .unwrap();
        assert_eq!(k.portrait.as_deref(), Some("/art/portraits/StarWyrm"));
        assert_eq!(k.animated.as_deref(), Some("/art/animated/StarWyrm"));
        let plain = Store::new(Catalog::new(fixture.0.join("boss")))
            .knowledge_v2(&dir, "PlainWyrm")
            .ok()
            .unwrap();
        assert_eq!(
            plain.portrait, None,
            "only event documents lend their key to art"
        );
    }

    /// An invented document whose `difficulty` runs a mission.
    fn mission_doc(key: &str, difficulty: &str, series: &str, order: u8) -> String {
        format!(
            "boss: {key}\nsummary: Invented.\ncore: [Invented.]\ndanger: [Invented.]\ntips: [Invented.]\n\
             difficulties:\n- name: Hard\n- name: {difficulty}\n  \
             mission: {{series: {series}, order: {order}, title: Invented mission}}\nsources: []\n"
        )
    }

    #[test]
    fn missions_list_the_doc_series_in_order() {
        let root = std::env::temp_dir().join(format!("pwa-mock-missions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (file, text) in [
            (
                "maleficstar.yaml",
                mission_doc("MaleficStar", "Destiny", "destiny-weapon", 2),
            ),
            (
                "kalos.yaml",
                mission_doc("Kalos", "Destiny", "destiny-weapon", 1),
            ),
            (
                "starwyrm.yaml",
                format!(
                    "{}event: {{name: Invented season, availability: Invented.}}\n",
                    mission_doc("StarWyrm", "Destiny", "destiny-weapon", 2)
                ),
            ),
            (
                "zenith.yaml",
                mission_doc("Zenith", "Champion", "union-champion", 1),
            ),
            ("quiet.yaml", "boss: Quiet\nsummary: Invented.\n".to_owned()),
            ("broken.yaml", "boss: [\n".to_owned()),
            (
                "ghost.yaml",
                mission_doc("Phantom", "Destiny", "destiny-weapon", 1),
            ),
        ] {
            std::fs::write(root.join(file), text).unwrap();
        }
        let dir = KnowledgeDir(root.clone());
        let missions = |key: &str| -> Vec<String> {
            store()
                .knowledge_v2(&dir, key)
                .ok()
                .unwrap()
                .missions
                .iter()
                .map(|stop| {
                    format!(
                        "{} {} {} {} {}",
                        stop["series"].as_str().unwrap(),
                        stop["order"],
                        stop["key"].as_str().unwrap(),
                        stop["name"].as_str().unwrap(),
                        stop["difficulty"].as_str().unwrap()
                    )
                })
                .collect()
        };
        let destiny = [
            "destiny-weapon 1 Kalos Gatekeeper Kalos Destiny",
            "destiny-weapon 2 MaleficStar Radiant Malefic Star Destiny",
            "destiny-weapon 2 StarWyrm StarWyrm Destiny",
        ];
        assert_eq!(missions("MaleficStar"), destiny);
        assert_eq!(missions("StarWyrm"), destiny);
        assert_eq!(
            missions("Zenith"),
            ["union-champion 1 Zenith Zenith Champion"]
        );
        assert!(missions("Quiet").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
