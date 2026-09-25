//! The frozen A0 contract (`docs/v5/api-schemas`), registered in memory under each file's `$id`.

use std::{fs, path::PathBuf};

use jsonschema::{Resource, Validator};
use serde_json::{Value, json};

const BASE: &str = "https://kanade.invalid/api-schemas/";

fn schemas() -> Vec<(String, Value)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/v5/api-schemas");
    fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{dir:?}: {error}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| {
            let schema: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                schema,
            )
        })
        .collect()
}

/// `target` is `file.json#/$defs/Name`.
pub fn validator(target: &str) -> Validator {
    jsonschema::options()
        .with_resources(
            schemas()
                .into_iter()
                .map(|(name, schema)| (format!("{BASE}{name}"), Resource::from_contents(schema))),
        )
        .build(&json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$ref": format!("{BASE}{target}"),
        }))
        .unwrap_or_else(|error| panic!("{target}: {error}"))
}

/// Records can carry any surface the domain has, so the schema lists them all.
#[test]
fn the_surface_enum_is_the_domains() {
    let common = schemas()
        .into_iter()
        .find(|(name, _)| name == "common.json")
        .unwrap()
        .1;
    let listed: Vec<&str> = common["$defs"]["Surface"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    let domain: Vec<&str> = kanade::domain::history::Surface::ALL
        .iter()
        .map(|surface| surface.as_str())
        .collect();
    assert_eq!(listed, domain);
}

/// Panics with every violation, naming the endpoint.
pub fn assert_valid(target: &str, what: &str, value: &Value) {
    let validator = validator(target);
    let errors: Vec<String> = validator
        .iter_errors(value)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{what} vs {target}:\n{}\n{value:#}",
        errors.join("\n")
    );
}
