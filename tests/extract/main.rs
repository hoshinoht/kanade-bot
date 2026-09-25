//! Replays the frozen v4 extraction vectors for the pure rules: gate, window,
//! resolve, match and merge. Prompt, parse, plan, commit and cards belong to
//! later slices.

mod gate;
mod matching;
mod merge;
mod resolve;
mod support;
mod window;

const REPLAYED: [&str; 5] = ["gate", "window", "resolve", "match", "merge"];

#[test]
fn index_lists_the_replayed_families_with_their_schemas() {
    let index = support::load("index.json");
    assert_eq!(index["schema_version"], "v5-extract-index-v1");
    let families = index["families"].as_array().expect("families");
    for family in REPLAYED {
        let entry = families
            .iter()
            .find(|entry| entry["family"] == family)
            .unwrap_or_else(|| panic!("index lacks {family}"));
        assert_eq!(entry["schema"], format!("{family}.schema.json"));
        assert_eq!(entry["vector"], format!("{family}.json"));
    }
}
