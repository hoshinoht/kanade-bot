//! `boss/knowledge/`: schema v2 documents, read per request by the API.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{LoadError, read::read_text};

/// Knowledge files are a few KiB; anything far larger is not ours.
const MAX_KNOWLEDGE_BYTES: u64 = 256 * 1024;
const SCHEMA_FILE: &str = "schema.json";
const META_FILE: &str = "_meta.yaml";
const SCHEMA_VERSION: u64 = 2;

/// One knowledge document as the API reads it.
pub(crate) fn read_document(path: &Path) -> Result<Value, LoadError> {
    let text = read_text(path, MAX_KNOWLEDGE_BYTES)?;
    serde_saphyr::from_str(&text)
        .map_err(|error| LoadError::new(path, format!("invalid YAML: {error}")))
}

/// An event boss document (one with an `event` block): its key and the
/// other names members use for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeEvent {
    pub key: String,
    pub aliases: Vec<String>,
}

/// A knowledge directory whose every document validated at startup.
#[derive(Clone, Debug)]
pub struct KnowledgeDir {
    pub path: PathBuf,
    /// Document `boss` keys, sorted.
    pub keys: Vec<String>,
    /// Event documents, in key order.
    pub events: Vec<KnowledgeEvent>,
}

impl KnowledgeDir {
    /// The document for catalog key `key` and `_meta.yaml`'s
    /// `researched_as_of`, re-read per call; `Ok(None)` when no document was
    /// validated for that key.
    pub fn guide_source(&self, key: &str) -> Result<Option<(Value, String)>, LoadError> {
        if !self.keys.iter().any(|known| known == key) {
            return Ok(None);
        }
        let meta_path = self.path.join(META_FILE);
        let meta = read_document(&meta_path)?;
        let researched = meta
            .get("researched_as_of")
            .and_then(Value::as_str)
            .ok_or_else(|| LoadError::new(&meta_path, "missing researched_as_of"))?
            .to_owned();
        let path = self.path.join(format!("{}.yaml", key.to_ascii_lowercase()));
        Ok(Some((read_document(&path)?, researched)))
    }
}

pub fn load_knowledge_dir(dir: &Path) -> Result<KnowledgeDir, LoadError> {
    if !fs::metadata(dir).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(LoadError::new(dir, "not a directory"));
    }
    let schema_path = dir.join(SCHEMA_FILE);
    let schema: Value = serde_json::from_str(&read_text(&schema_path, MAX_KNOWLEDGE_BYTES)?)
        .map_err(|error| LoadError::new(&schema_path, format!("invalid JSON: {error}")))?;
    let validator = jsonschema::validator_for(&schema)
        .map_err(|error| LoadError::new(&schema_path, format!("invalid schema: {error}")))?;

    let meta_path = dir.join(META_FILE);
    let meta = read_document(&meta_path)?;
    if meta.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err(LoadError::new(
            &meta_path,
            format!("schema_version must be {SCHEMA_VERSION}"),
        ));
    }

    let mut names: Vec<String> = fs::read_dir(dir)
        .map_err(|error| LoadError::new(dir, error.to_string()))?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".yaml") && !name.starts_with('_'))
        .collect();
    names.sort();
    let mut keys = Vec::with_capacity(names.len());
    let mut events = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let doc = read_document(&path)?;
        if let Some(error) = validator.iter_errors(&doc).next() {
            let at = error.instance_path().to_string();
            let at = if at.is_empty() { "/".to_owned() } else { at };
            return Err(LoadError::new(
                &path,
                format!("schema violation at {at} (rule {})", error.schema_path()),
            ));
        }
        let key = doc.get("boss").and_then(Value::as_str).unwrap_or_default();
        // The API finds a document only through the lowercased key.
        if name.strip_suffix(".yaml") != Some(key.to_ascii_lowercase().as_str()) {
            return Err(LoadError::new(
                &path,
                "file name must be the lowercased `boss` key",
            ));
        }
        if let Some(event) = doc.get("event") {
            let aliases = event
                .get("aliases")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            events.push(KnowledgeEvent {
                key: key.to_owned(),
                aliases,
            });
        }
        keys.push(key.to_owned());
    }
    Ok(KnowledgeDir {
        path: dir.to_path_buf(),
        keys,
        events,
    })
}
