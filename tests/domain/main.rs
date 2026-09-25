//! Replays the frozen v4 pure-domain vectors through the Rust domain API.

mod bosses;
mod ids;
mod support;
mod timeutil;
mod weeks;

use serde_json::Value;

const FAMILIES: [&str; 4] = ["weeks", "timeutil", "ids", "bosses"];

#[test]
fn index_lists_exactly_the_replayed_families() {
    let index = support::load("index.json");
    assert_eq!(index["schema_version"], "v5-domain-v1");
    let listed: Vec<&str> = index["vectors"]
        .as_array()
        .expect("vectors array")
        .iter()
        .map(|name| name.as_str().expect("vector file name"))
        .collect();
    let expected: Vec<String> = FAMILIES
        .iter()
        .map(|family| format!("{family}.json"))
        .collect();
    assert_eq!(listed, expected);
}

#[test]
fn vector_files_satisfy_their_schema() {
    let schema: Value = support::load("schema.json");
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    for family in FAMILIES {
        let file = support::load(&format!("{family}.json"));
        let errors: Vec<String> = validator
            .iter_errors(&file)
            .map(|error| error.to_string())
            .collect();
        assert!(errors.is_empty(), "{family}.json: {errors:?}");
    }
}
