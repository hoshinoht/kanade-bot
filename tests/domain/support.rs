//! Loads frozen v4 domain vectors and compares replayed outcomes exactly.

use std::{fs, path::PathBuf};

use chrono::{DateTime, FixedOffset, NaiveTime, Timelike};
use chrono_tz::Tz;
use kanade::domain::time::IsoDateTime;
use serde_json::Value;

/// A replayed success value, or the Python error class and full message.
pub type Outcome = Result<Value, (&'static str, String)>;

pub fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/v5/vectors/domain")
}

pub fn load(name: &str) -> Value {
    let path = vector_dir().join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path:?}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path:?}: {error}"))
}

/// Replay every case of one family file; any skipped, unknown or differing case fails.
pub fn replay_family(family: &str, replay: impl Fn(&str, &Value, &Value) -> Outcome) {
    let file = load(&format!("{family}.json"));
    assert_eq!(file["family"], family);
    assert_eq!(file["schema_version"], "v5-domain-v1");
    let fixtures = file.get("fixtures").cloned().unwrap_or(Value::Null);
    let cases = file["cases"].as_array().expect("cases array");
    assert!(!cases.is_empty(), "{family} has no cases");
    let mut replayed = 0;
    let mut failures = Vec::new();
    for case in cases {
        let id = case["case_id"].as_str().expect("case_id");
        let op = case["op"].as_str().expect("op");
        let actual = match replay(op, &case["input"], &fixtures) {
            Ok(value) => serde_json::json!({ "value": value }),
            Err((kind, message)) => {
                serde_json::json!({ "error": { "type": kind, "message": message } })
            }
        };
        if actual != case["expected"] {
            failures.push(format!("{id}: expected {} got {actual}", case["expected"]));
        }
        replayed += 1;
    }
    assert!(
        failures.is_empty(),
        "{family} mismatches:\n{}",
        failures.join("\n")
    );
    assert_eq!(replayed, cases.len(), "{family} skipped cases");
}

pub fn unknown_op(family: &str, op: &str) -> ! {
    panic!("unknown {family} vector operation {op:?}")
}

pub fn text<'a>(input: &'a Value, key: &str) -> &'a str {
    input[key]
        .as_str()
        .unwrap_or_else(|| panic!("input.{key} must be a string"))
}

pub fn integer(input: &Value, key: &str) -> i64 {
    input[key]
        .as_i64()
        .unwrap_or_else(|| panic!("input.{key} must be an integer"))
}

pub fn zone(input: &Value) -> Tz {
    text(input, "timezone").parse().expect("IANA timezone")
}

/// Python `datetime.fromisoformat`, possibly naive.
pub fn iso(input: &Value, key: &str) -> IsoDateTime {
    IsoDateTime::parse(text(input, key)).expect("vector datetime")
}

pub fn instant(input: &Value, key: &str) -> DateTime<FixedOffset> {
    match iso(input, key) {
        IsoDateTime::Aware(at) => at,
        IsoDateTime::Naive(_) => panic!("input.{key} must be aware"),
    }
}

/// Python `time.fromisoformat` for the `HH:MM:SS` vectors use.
pub fn clock(input: &Value, key: &str) -> NaiveTime {
    let parts: Vec<u32> = text(input, key)
        .split(':')
        .map(|part| part.parse().expect("clock digits"))
        .collect();
    let [hour, minute, second] = parts[..] else {
        panic!("input.{key} must be HH:MM:SS");
    };
    NaiveTime::from_hms_opt(hour, minute, second).expect("valid clock")
}

/// Python `time.isoformat()` for whole-second times.
pub fn clock_text(time: NaiveTime) -> String {
    assert_eq!(time.nanosecond(), 0, "vectors carry whole seconds");
    format!(
        "{:02}:{:02}:{:02}",
        time.hour(),
        time.minute(),
        time.second()
    )
}
