//! Loads the frozen v4 extraction vectors, schema-gates them, and compares
//! replayed step outcomes exactly.

use std::{fs, path::PathBuf};

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, Timelike};
use chrono_tz::Tz;
use kanade::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec};
use kanade::domain::schedule::RsvpState;
use kanade::domain::time::IsoDateTime;
use kanade::extract::{Amendment, AmendmentKind};
use serde_json::{Value, json};

/// A replayed success value, or the declared Python error class and message.
pub type Outcome = Result<Value, (&'static str, String)>;

const DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

pub fn load(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("docs/v5/vectors/extract")
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

/// Replay every step of every case in `<family>.json`.
///
/// The document is validated against its schema, each case's input against
/// `$defs/replayCase` (as the v4 replayer gates it), and each replayed value
/// against `$defs/result_<op>`. Returns `(cases, steps)` replayed; any
/// mismatch, skip or undeclared error fails.
pub fn replay_family(family: &str, replay: impl Fn(&Value, &Value) -> Outcome) -> (usize, usize) {
    let file = load(&format!("{family}.json"));
    let schema = load(&format!("{family}.schema.json"));
    let document = validator(&schema, None);
    let problems = errors(&document, &file);
    assert!(problems.is_empty(), "{family}.json: {problems:?}");
    assert_eq!(file["family"], family);
    assert_eq!(file["schema_version"], format!("v5-extract-{family}-v1"));
    let replay_case = validator(&schema, Some("replayCase"));

    let cases = file["cases"].as_array().expect("cases");
    assert!(!cases.is_empty(), "{family} has no cases");
    let (mut replayed_cases, mut replayed_steps) = (0, 0);
    let mut failures = Vec::new();
    for case in cases {
        let id = text(&case["case_id"]);
        let gated = json!({ "case_id": case["case_id"], "input": case["input"] });
        let problems = errors(&replay_case, &gated);
        assert!(problems.is_empty(), "{id}: input gate: {problems:?}");
        let input = &case["input"];
        let steps = input["steps"].as_array().expect("steps");
        let expected = case["expected"]["steps"]
            .as_array()
            .expect("expected steps");
        assert_eq!(steps.len(), expected.len(), "{id}: step count");
        for (index, (step, want)) in steps.iter().zip(expected).enumerate() {
            let op = text(&step["op"]);
            let got = match replay(input, step) {
                Ok(value) => {
                    let result = validator(&schema, Some(&format!("result_{op}")));
                    let problems = errors(&result, &value);
                    assert!(
                        problems.is_empty(),
                        "{id} step {index}: result_{op}: {problems:?}"
                    );
                    json!({ "value": value })
                }
                Err((kind, message)) => {
                    assert_eq!(
                        step["error_type"], kind,
                        "{id} step {index}: undeclared error"
                    );
                    json!({ "error": { "type": kind, "message": message } })
                }
            };
            if &got != want {
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
    (replayed_cases, replayed_steps)
}

pub fn unknown_op(family: &str, op: &str) -> ! {
    panic!("unknown {family} vector operation {op:?}")
}

pub fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("{value} must be a string"))
}

pub fn opt_text(value: &Value) -> Option<&str> {
    if value.is_null() {
        None
    } else {
        Some(text(value))
    }
}

pub fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("{value} must be an array"))
        .iter()
        .map(|item| text(item).to_owned())
        .collect()
}

pub fn flag(value: &Value) -> bool {
    value
        .as_bool()
        .unwrap_or_else(|| panic!("{value} must be a boolean"))
}

pub fn zone(input: &Value) -> Tz {
    text(&input["timezone"]).parse().expect("IANA timezone")
}

pub fn instant(value: &Value) -> DateTime<FixedOffset> {
    match IsoDateTime::parse(text(value)).expect("vector datetime") {
        IsoDateTime::Aware(at) => at,
        IsoDateTime::Naive(_) => panic!("{value} must be aware"),
    }
}

pub fn date(value: &Value) -> NaiveDate {
    text(value).parse().expect("vector date")
}

/// `HH:MM:SS`, Python `time.isoformat()` for whole seconds.
pub fn clock_iso(time: NaiveTime) -> String {
    assert_eq!(time.nanosecond(), 0, "sub-second clock");
    time.to_string()
}

/// Rebuild the fixture catalog as v4's `BossTable.from_dict` does.
pub fn catalog(raw: &Value) -> BossTable {
    let difficulties = raw["difficulties"]
        .as_array()
        .expect("difficulties")
        .iter()
        .map(|entry| DifficultySpec {
            prefix: text(&entry["prefix"]).to_owned(),
            label: text(&entry["label"]).to_owned(),
        })
        .collect();
    let bosses = raw["bosses"]
        .as_array()
        .expect("bosses")
        .iter()
        .map(|entry| BossSpec {
            short: text(&entry["short"]).to_owned(),
            full: Some(text(&entry["full"]).to_owned()),
            difficulties: entry.get("difficulties").map(strings),
            aliases: strings(&entry["aliases"]),
            ..BossSpec::default()
        })
        .collect();
    BossTable::from_spec(&CatalogSpec {
        difficulties,
        bosses,
    })
    .expect("fixture catalog is valid")
}

/// A vector amendment. Inputs are already canonical v4 dumps, so this reads
/// them without coercion and [`amendment_json`] must give them back unchanged.
pub fn amendment(raw: &Value) -> Amendment {
    let rsvp = opt_text(&raw["rsvp"]).map(|state| RsvpState::parse(state).expect("rsvp state"));
    let parsed = Amendment {
        kind: AmendmentKind::parse(text(&raw["kind"])).expect("amendment kind"),
        bosses: strings(&raw["bosses"]),
        day_ref: opt_text(&raw["day_ref"]).map(str::to_owned),
        time_ref: opt_text(&raw["time_ref"]).map(str::to_owned),
        participants: strings(&raw["participants"]),
        rsvp,
        is_question: flag(&raw["is_question"]),
        confidence: raw["confidence"].as_f64().expect("confidence"),
        evidence_message_ids: strings(&raw["evidence_message_ids"]),
        target_run_hint: opt_text(&raw["target_run_hint"]).map(str::to_owned),
    };
    assert_eq!(
        &amendment_json(&parsed),
        raw,
        "amendment input is not canonical"
    );
    parsed
}

/// v4 `Amendment.model_dump(mode="json")`.
pub fn amendment_json(value: &Amendment) -> Value {
    json!({
        "kind": value.kind.as_str(),
        "bosses": value.bosses,
        "day_ref": value.day_ref,
        "time_ref": value.time_ref,
        "participants": value.participants,
        "rsvp": value.rsvp.map(RsvpState::as_str),
        "is_question": value.is_question,
        "confidence": value.confidence,
        "evidence_message_ids": value.evidence_message_ids,
        "target_run_hint": value.target_run_hint,
    })
}
