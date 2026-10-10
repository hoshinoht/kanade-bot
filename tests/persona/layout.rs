use std::{fs, path::Path};

use kanade::chat::persona::{PersonaSnapshot, SelectionSource};

use crate::support::{Fixture, pid, tracked};

#[test]
fn tracked_files_alone_start_on_kanade_with_the_example_catalog() {
    let fixture = Fixture::empty();
    fixture.write("bundles/kanade.yaml", tracked("bundles/kanade.yaml"));
    fixture.write("profiles/example.yaml", tracked("profiles/example.yaml"));
    let snapshot = PersonaSnapshot::startup(&fixture.root(), Some(&pid("kanade")));
    assert_eq!(
        snapshot.provenance().source,
        Some(SelectionSource::TrackedFallback)
    );
    let active = snapshot.active().unwrap();
    assert!(active.profiles.readable.is_empty() && active.profiles.unreadable.is_empty());

    fixture.write("catalog.yaml", tracked("catalog.example.yaml"));
    let snapshot = PersonaSnapshot::startup(&fixture.root(), None);
    assert_eq!(
        snapshot.provenance().source,
        Some(SelectionSource::CatalogDefault)
    );
    assert_eq!(snapshot.provenance().effective, Some(pid("kanade")));
}

fn rust_sources(dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push((
                path.display().to_string(),
                fs::read_to_string(&path).unwrap(),
            ));
        }
    }
}

#[test]
fn runtime_names_no_legacy_persona_paths() {
    let mut sources = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    assert!(sources.iter().any(|(path, _)| path.ends_with("loader.rs")));
    for (path, text) in sources {
        for needle in [
            "legacy/",
            "personas.yaml",
            "identity.md",
            "default.md",
            "staging.yaml",
            "behaviours",
            "default-compact",
        ] {
            assert!(!text.contains(needle), "{path} mentions {needle}");
        }
    }
}
