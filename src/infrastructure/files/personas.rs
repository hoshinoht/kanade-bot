//! `config/personas/` → startup snapshot plus the profile options the API lists.

use std::path::Path;

use super::LoadError;
use crate::{
    api::state::PersonaOption,
    chat::persona::{PersonaId, PersonaRoot, PersonaSnapshot},
};

#[derive(Debug)]
pub struct PersonaLoad {
    /// Startup selection with the loader's trusted Kanade fallback; inactive
    /// (chat disabled) when nothing validates.
    pub snapshot: PersonaSnapshot,
    /// Readable reply profiles in ID order; empty while chat is disabled.
    pub options: Vec<PersonaOption>,
}

pub fn load_personas(dir: &Path, configured: Option<&PersonaId>) -> Result<PersonaLoad, LoadError> {
    let root = PersonaRoot::open(dir).map_err(|error| LoadError::new(dir, error.to_string()))?;
    let snapshot = PersonaSnapshot::startup(&root, configured);
    let options = snapshot
        .active()
        .map(|active| {
            active
                .profiles
                .readable
                .values()
                .map(|profile| PersonaOption {
                    key: profile.value.id.to_string(),
                    name: profile.value.label.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(PersonaLoad { snapshot, options })
}
