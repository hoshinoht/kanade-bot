//! Synthetic guides only: invented text, invented catalog, in-memory art.

mod blocks;
mod plan;
mod props;
mod render;
mod support;

use std::path::Path;

/// Every `.rs` file under `dir` whose text contains one of `names`.
fn naming(dir: &Path, names: &[&str], hits: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            naming(&path, names, hits);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            if names.iter().any(|name| text.contains(name)) {
                hits.push(path.display().to_string());
            }
        }
    }
}

/// Guide content never reaches a model: the chat and extraction code that
/// builds model prompts must not read it.
#[test]
fn chat_and_extraction_never_read_guides() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    for dir in ["chat", "extract"] {
        naming(
            &src.join(dir),
            &[
                "domain::guides",
                "GuideStore",
                "guide_slots",
                "RenderedSlot",
            ],
            &mut hits,
        );
    }
    assert!(hits.is_empty(), "guides named in {hits:?}");
}

/// Guide tests never read private guides or art (plan 9, privacy guard).
#[test]
fn guide_tests_never_name_private_paths() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/domain/guides/tests");
    let private = ["docs", "/notes"].concat();
    let names = [
        private.as_str(),
        &["config", "/guide"].concat(),
        &["boss", "/portraits"].concat(),
        &["boss", "/artwork"].concat(),
    ];
    let mut hits = Vec::new();
    naming(&dir, &names, &mut hits);
    assert!(hits.is_empty(), "private paths named in {hits:?}");
}
