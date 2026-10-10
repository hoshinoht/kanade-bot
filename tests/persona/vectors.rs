use std::{collections::BTreeSet, fs, path::Path};

use kanade::chat::persona::{PersonaId, PersonaRoot};
use serde_json::Value;

fn read_json(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs/v5/vectors/persona")
        .join(name);
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn oracle_vectors_parse_and_reference_tracked_bundles() {
    let document = read_json("persona.json");
    let schema = read_json("schema.json");
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&document));
    assert_eq!(document["schema_version"], "v5-persona-v1");

    let root = PersonaRoot::open(&crate::support::tracked_dir()).unwrap();
    let mut ids = BTreeSet::new();
    for recorded in document["bundles"].as_array().unwrap() {
        let id = PersonaId::parse(recorded["id"].as_str().unwrap()).unwrap();
        let bundle = root.load_bundle(&id).unwrap().value;
        assert_eq!(bundle.identity, recorded["identity"].as_str().unwrap());
        assert_eq!(
            bundle.prompt,
            recorded["behaviour_prompt"].as_str().unwrap()
        );
        let staging = &recorded["staging"];
        for (key, line) in [
            ("schedule", &bundle.staging.schedule),
            ("guide", &bundle.staging.guide),
            ("guide_named", &bundle.staging.guide_named),
            ("write", &bundle.staging.write),
            ("generic", &bundle.staging.generic),
        ] {
            assert_eq!(staging[key].as_str().unwrap(), line, "{key}");
        }
        ids.insert(id);
    }
    let cases = document["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let id = PersonaId::parse(case["input"]["bundle_id"].as_str().unwrap()).unwrap();
        assert!(ids.contains(&id), "{}", case["case_id"]);
        assert!(
            !case["expected"]["system_prompt"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }
}
