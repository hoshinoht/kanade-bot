//! Loads the frozen v4 chat vectors, schema-gates them, applies named v5
//! deviations and compares every replayed step exactly.

use std::{fs, future::Future, path::PathBuf};

use serde_json::{Value, json};

use crate::common::{Deviation, apply_deviations};

const DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

pub fn load(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("docs/v5/vectors/chat")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path:?}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path:?}: {error}"))
}

fn validator(schema: &Value, target: Option<&str>) -> jsonschema::Validator {
    let schema = match target {
        None => schema.clone(),
        Some(target) => json!({
            "$schema": DRAFT,
            "$defs": schema["$defs"],
            "$ref": format!("#/$defs/{target}"),
        }),
    };
    jsonschema::validator_for(&schema).expect("schema compiles")
}

fn errors(validator: &jsonschema::Validator, instance: &Value) -> Vec<String> {
    validator
        .iter_errors(instance)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect()
}

/// A named group of deliberate v5 differences; every entry must apply.
pub struct Named {
    pub name: &'static str,
    pub entries: Vec<Deviation>,
}

pub fn dev(case_id: &'static str, pointer: impl Into<String>, v4: Value, v5: Value) -> Deviation {
    Deviation {
        case_id,
        pointer: pointer.into(),
        v4,
        v5,
    }
}

/// A replayed step: `{value}` or `{error: {type, message}}`.
pub fn value(value: Value) -> Value {
    json!({ "value": value })
}

pub fn error(kind: &str, message: impl Into<String>) -> Value {
    json!({ "error": { "type": kind, "message": message.into() } })
}

pub fn unknown_op(family: &str, op: &str) -> ! {
    panic!("unknown {family} vector operation {op:?}")
}

/// Replay every case of `<family>.json` with `replay` (one call per case,
/// returning its step results) and compare them to the expectation after
/// `named` deviations. The document, each case's input (`replayCase`) and
/// each replayed value (`result_<op>`) are schema-checked. Returns
/// `(cases, steps)`.
pub async fn check_family<F, Fut>(family: &str, named: &[Named], replay: F) -> (usize, usize)
where
    F: Fn(Value) -> Fut,
    Fut: Future<Output = Vec<Value>>,
{
    let file = load(&format!("{family}.json"));
    let schema = load(&format!("{family}.schema.json"));
    let problems = errors(&validator(&schema, None), &file);
    assert!(problems.is_empty(), "{family}.json: {problems:?}");
    assert_eq!(file["family"], family);
    assert_eq!(
        file["schema_version"],
        format!("v5-chat-{}-v1", family.replace('_', "-"))
    );
    let gate = validator(&schema, Some("replayCase"));
    let cases = file["cases"].as_array().expect("cases");
    assert!(!cases.is_empty(), "{family} has no cases");
    let mut used = vec![0; named.len()];
    let (mut replayed_cases, mut replayed_steps) = (0, 0);
    let mut failures = Vec::new();
    for case in cases {
        let id = case["case_id"].as_str().expect("case id");
        let gated = json!({ "case_id": case["case_id"], "input": case["input"] });
        let problems = errors(&gate, &gated);
        assert!(problems.is_empty(), "{id}: input gate: {problems:?}");
        let mut expected = case["expected"].clone();
        let mut deviated = Vec::new();
        for (slot, group) in named.iter().enumerate() {
            let (next, applied) = apply_deviations(id, &expected, &group.entries);
            expected = next;
            used[slot] += applied;
            deviated.extend(
                group
                    .entries
                    .iter()
                    .filter(|entry| entry.case_id == id)
                    .map(|entry| entry.pointer.clone()),
            );
        }
        let steps = case["input"]["steps"].as_array().expect("steps");
        let want = expected["steps"]
            .as_array()
            .expect("expected steps")
            .clone();
        let got = replay(case.clone()).await;
        assert_eq!(got.len(), steps.len(), "{id}: replayed step count");
        assert_eq!(want.len(), steps.len(), "{id}: expected step count");
        for (index, ((step, got), want)) in steps.iter().zip(&got).zip(&want).enumerate() {
            let op = step["op"].as_str().expect("op");
            if let Some(value) = got.get("value") {
                let touched = deviated
                    .iter()
                    .any(|pointer| pointer.starts_with(&format!("/steps/{index}/")));
                let result = validator(&schema, Some(&format!("result_{op}")));
                if !touched || errors(&result, &want["value"]).is_empty() {
                    let problems = errors(&result, value);
                    assert!(
                        problems.is_empty(),
                        "{id} step {index}: result_{op}: {problems:?}"
                    );
                }
            } else {
                assert_eq!(
                    step["error_type"], got["error"]["type"],
                    "{id} step {index}: undeclared error {got}"
                );
            }
            if got != want {
                failures.push(format!(
                    "{id} step {index} ({op}): expected {want}\n  got {got}"
                ));
            }
            replayed_steps += 1;
        }
        replayed_cases += 1;
    }
    assert!(
        failures.is_empty(),
        "{family} mismatches:\n{}",
        failures.join("\n")
    );
    assert_eq!(replayed_cases, cases.len(), "{family} skipped cases");
    for (group, used) in named.iter().zip(used) {
        assert_eq!(
            used,
            group.entries.len(),
            "{}: deviation unused",
            group.name
        );
    }
    (replayed_cases, replayed_steps)
}
