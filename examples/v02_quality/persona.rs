//! The persona bundle under test: one operator-chosen bundle file (the
//! tracked `kanade.yaml` by default), validated with the production schema.
//! Only its id, file name and digest are ever recorded, never its text.

use std::path::{Path, PathBuf};

use kanade::chat::persona::{Bundle, PersonaId, parse_bundle};

pub struct PersonaBundle {
    pub path: PathBuf,
    pub file_name: String,
    pub id: PersonaId,
    pub sha256: String,
    pub bundle: Bundle,
}

pub fn tracked() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("config/personas/bundles/kanade.yaml")
}

/// Bundles live at `bundles/<id>.yaml`, so the file name gives the id the
/// schema checks the bundle's own `id` against.
pub fn load(path: &Path) -> Result<PersonaBundle, String> {
    let shown = path.display();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(format!("{shown}: not a file name"))?
        .to_owned();
    let stem = file_name
        .strip_suffix(".yaml")
        .ok_or(format!("{shown}: a bundle file is named <id>.yaml"))?;
    let id = PersonaId::parse(stem).map_err(|e| format!("{shown}: {e}"))?;
    let bytes = std::fs::read(path).map_err(|e| format!("{shown}: {e}"))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| format!("{shown}: not UTF-8"))?;
    let bundle = parse_bundle(text, &id).map_err(|e| format!("{shown}: {e}"))?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let sha256 = digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(PersonaBundle {
        path: path.to_owned(),
        file_name,
        id,
        sha256,
        bundle,
    })
}
