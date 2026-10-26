//! `config/personas/` → startup snapshot plus the profile options the API lists.

use std::path::Path;

use super::{PersonaId, PersonaRoot, PersonaSnapshot};
use kanade_files::LoadError;

/// A reply-style profile members may choose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonaOption {
    pub key: String,
    pub name: String,
    /// The profile's one-line voice; empty when it has none.
    pub voice: String,
}

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
                    voice: profile.value.voice.clone().unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(PersonaLoad { snapshot, options })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::load_personas;

    fn repo(path: &str) -> PathBuf {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).join(path)
    }

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("kanade-personas-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// `Personas/` holding only the tracked `kanade` bundle.
    fn personas_root(temp: &Temp) -> PathBuf {
        let root = temp.0.join("Personas");
        fs::create_dir_all(root.join("bundles")).expect("dirs");
        fs::copy(
            repo("config/personas/bundles/kanade.yaml"),
            root.join("bundles/kanade.yaml"),
        )
        .expect("bundle");
        root
    }

    #[test]
    fn personas_fall_back_to_tracked_kanade_without_private_files() {
        let temp = Temp::new();
        let root = personas_root(&temp);
        let load = load_personas(&root, None).expect("personas");
        let active = load.snapshot.active().expect("fallback active");
        assert_eq!(active.bundle.value.id.to_string(), "kanade");
        assert!(load.options.is_empty());
    }

    #[test]
    fn personas_list_readable_profiles_but_never_the_example() {
        let temp = Temp::new();
        let root = personas_root(&temp);
        fs::create_dir_all(root.join("profiles")).expect("dir");
        fs::copy(
            repo("config/personas/catalog.example.yaml"),
            root.join("catalog.yaml"),
        )
        .expect("catalog");
        let example =
            fs::read_to_string(repo("config/personas/profiles/example.yaml")).expect("example");
        fs::write(root.join("profiles/example.yaml"), &example).expect("example");
        fs::write(
            root.join("profiles/calm.yaml"),
            example.replace("id: example", "id: calm"),
        )
        .expect("calm");
        fs::write(root.join("profiles/broken.yaml"), "id: [\n").expect("broken");
        let load = load_personas(&root, None).expect("personas");
        let keys: Vec<_> = load
            .options
            .iter()
            .map(|option| option.key.as_str())
            .collect();
        assert_eq!(keys, ["calm"]);
    }
}
