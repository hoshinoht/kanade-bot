//! Replays the frozen v4 mention, dispatch and digest vectors through the pure
//! v5 notification core, with the scheduler service and in-memory store holding
//! rows and a scripted journal/transport standing in for delivery.

mod attendance;
mod digest_family;
mod dispatch_family;
mod harness;
mod invariants;
mod mentions_family;

// Vector loading, pinned clock/ids and snapshots shared with the scheduler target.
#[path = "../common/mod.rs"]
mod common;

/// Families this target replays.
const REPLAYED: [(&str, &str, &str); 3] = [
    ("mentions", "mentions.schema.json", "mentions.json"),
    ("dispatch", "dispatch.schema.json", "dispatch.json"),
    ("digest", "digest.schema.json", "digest.json"),
];

#[test]
fn index_lists_the_replayed_families_with_their_schemas() {
    let index = common::load("index.json");
    let families = index["families"].as_array().expect("families");
    for (family, schema, vector) in REPLAYED {
        let entry = families
            .iter()
            .find(|entry| entry["family"] == family)
            .unwrap_or_else(|| panic!("index lacks {family}"));
        assert_eq!(entry["schema"], schema, "{family}");
        assert_eq!(entry["vector"], vector, "{family}");
    }
}

#[test]
fn vector_files_satisfy_their_schemas() {
    for (family, schema, vector) in REPLAYED {
        let schema = common::load(schema);
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");
        let file = common::load(vector);
        let errors: Vec<String> = validator
            .iter_errors(&file)
            .map(|error| error.to_string())
            .collect();
        assert!(errors.is_empty(), "{family}: {errors:?}");
    }
}
