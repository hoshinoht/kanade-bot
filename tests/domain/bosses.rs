use kanade::domain::catalog::{BossParseError, BossSpec, BossTable, CatalogSpec, DifficultySpec};
use serde_json::{Map, Value, json};

use crate::support::{Outcome, replay_family, text, unknown_op};

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("string array")
        .iter()
        .map(|item| item.as_str().expect("string").to_owned())
        .collect()
}

fn fields<'a>(entry: &'a Value, allowed: &[&str]) -> &'a Map<String, Value> {
    let map = entry.as_object().expect("catalog entry object");
    for key in map.keys() {
        assert!(
            allowed.contains(&key.as_str()),
            "unexpected catalog key {key:?}"
        );
    }
    map
}

/// The ordered-array fixture as the decoded spec a loader would produce.
fn catalog(fixtures: &Value, input: &Value) -> BossTable {
    let raw = &fixtures["catalogs"][text(input, "catalog_id")];
    let difficulties = raw["difficulties"]
        .as_array()
        .expect("difficulties array")
        .iter()
        .map(|entry| {
            let entry = fields(entry, &["prefix", "label"]);
            DifficultySpec {
                prefix: entry["prefix"].as_str().expect("prefix").to_owned(),
                label: entry["label"].as_str().expect("label").to_owned(),
            }
        })
        .collect();
    let bosses = raw["bosses"]
        .as_array()
        .expect("bosses array")
        .iter()
        .map(|entry| {
            let entry = fields(
                entry,
                &["short", "full", "level", "difficulties", "aliases"],
            );
            BossSpec {
                short: entry["short"].as_str().expect("short").to_owned(),
                full: entry
                    .get("full")
                    .map(|full| full.as_str().expect("full").to_owned()),
                level: entry
                    .get("level")
                    .map(|level| level.as_i64().expect("level")),
                difficulties: entry.get("difficulties").map(strings),
                aliases: entry.get("aliases").map(strings).unwrap_or_default(),
                ..BossSpec::default()
            }
        })
        .collect();
    BossTable::from_spec(&CatalogSpec {
        difficulties,
        bosses,
    })
    .expect("synthetic catalog is valid")
}

fn replay(op: &str, input: &Value, fixtures: &Value) -> Outcome {
    let table = catalog(fixtures, input);
    let parse_error = |error: BossParseError| ("BossParseError", error.to_string());
    match op {
        "parse_token" => Ok(json!(
            table
                .parse_token(text(input, "token"))
                .map_err(parse_error)?
        )),
        "parse" => Ok(json!(
            table.parse(text(input, "text")).map_err(parse_error)?
        )),
        "resolve_reference" => {
            let reference = table
                .resolve_reference(text(input, "text"))
                .map_err(parse_error)?;
            Ok(json!({ "short": reference.short, "difficulty": reference.difficulty }))
        }
        "names_in" => Ok(json!(table.names_in(text(input, "text")))),
        "describe" => Ok(json!(table.describe(text(input, "canonical")))),
        "detail" => Ok(match table.detail(text(input, "canonical")) {
            None => Value::Null,
            Some(detail) => json!({
                "token": detail.token,
                "short": detail.short,
                "full": detail.full,
                "level": detail.level,
                "letter": detail.letter,
                "difficulty": detail.difficulty,
            }),
        }),
        other => unknown_op("bosses", other),
    }
}

#[test]
fn bosses_vectors_replay_exactly() {
    replay_family("bosses", replay);
}
