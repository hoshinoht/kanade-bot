//! Loads the frozen v4 extraction vectors, schema-gates them, and compares
//! replayed step outcomes exactly.

use std::{fs, path::PathBuf};

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, Timelike, Utc};
use chrono_tz::Tz;
use kanade::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec};
use kanade::domain::schedule::{RsvpState, Run, RunSource, RunStatus};
use kanade::domain::time::IsoDateTime;
use kanade::extract::resolve::Resolved;
use kanade::extract::schema::Extraction;
use kanade::extract::{Amendment, AmendmentKind};
use kanade::infrastructure::llm::{ChatRequest, Effort, Message, ModelCapabilities, wire_body};
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

/// A deliberate v5 difference from one frozen v4 step value. `rewrite` asserts
/// the v4 value it replaces and returns how many places it changed; an entry
/// that changes nothing, or is never reached, fails the replay.
pub struct Deviation {
    pub name: &'static str,
    pub case_id: &'static str,
    pub step: usize,
    pub rewrite: fn(&mut Value) -> usize,
}

/// Replay every step of every case in `<family>.json`.
///
/// The document is validated against its schema, each case's input against
/// `$defs/replayCase` (as the v4 replayer gates it), and each replayed value
/// against `$defs/result_<op>`. Returns `(cases, steps)` replayed; any
/// mismatch, skip or undeclared error fails.
pub fn replay_family(family: &str, replay: impl Fn(&Value, &Value) -> Outcome) -> (usize, usize) {
    replay_family_with(family, &[], |_, _| {}, replay)
}

/// [`replay_family`] with named deviations applied to the expected values and
/// `portable` applied to both sides of a step (only for fields the vector
/// README declares non-portable).
pub fn replay_family_with(
    family: &str,
    deviations: &[Deviation],
    portable: fn(&str, &mut Value),
    replay: impl Fn(&Value, &Value) -> Outcome,
) -> (usize, usize) {
    let mut used = vec![false; deviations.len()];
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
            let mut want = want.clone();
            let mut deviated = false;
            for (slot, deviation) in deviations.iter().enumerate() {
                if deviation.case_id != id || deviation.step != index {
                    continue;
                }
                let value = want
                    .get_mut("value")
                    .unwrap_or_else(|| panic!("{}: step has no value", deviation.name));
                let changed = (deviation.rewrite)(value);
                assert!(changed > 0, "{}: frozen v4 value not found", deviation.name);
                used[slot] = true;
                deviated = true;
            }
            let got = match replay(input, step) {
                Ok(mut value) => {
                    let result = validator(&schema, Some(&format!("result_{op}")));
                    // A deviated value is still checked unless the v5 value
                    // itself is outside the v4 result shape (a refusal).
                    if !deviated || errors(&result, &want["value"]).is_empty() {
                        let problems = errors(&result, &value);
                        assert!(
                            problems.is_empty(),
                            "{id} step {index}: result_{op}: {problems:?}"
                        );
                    }
                    portable(op, &mut value);
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
            if let Some(value) = want.get_mut("value") {
                portable(op, value);
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
    for (deviation, used) in deviations.iter().zip(used) {
        assert!(used, "{}: deviation never applied", deviation.name);
    }
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

/// v4 `Extraction.model_dump(mode="json")`.
pub fn extraction_json(value: &Extraction) -> Value {
    json!({
        "amendments": value.amendments.iter().map(amendment_json).collect::<Vec<_>>(),
        "summary": value.summary,
    })
}

/// The vectors' `dump_resolved`.
pub fn resolved_json(value: &Resolved) -> Value {
    json!({
        "day": value.day.map(|day| day.to_string()),
        "clock": value.clock.map(clock_iso),
        "at": value.at.map(|at| at.isoformat()),
        "assumed_pm": value.assumed_pm,
        "known": value.known(),
    })
}

/// A fixture run as a stored v5 row; `source` and `attendance` are not read
/// by extraction.
pub fn run(raw: &Value) -> Run {
    let utc = |value: &Value| instant(value).with_timezone(&Utc);
    Run {
        id: text(&raw["id"]).to_owned(),
        fixed_run_id: None,
        channel_id: opt_text(&raw["channel_id"]).map(str::to_owned),
        week_start: utc(&raw["week_start"]),
        datetime: utc(&raw["datetime"]),
        bosses: strings(&raw["bosses"]),
        participants: strings(&raw["participants"]),
        status: RunStatus::parse(text(&raw["status"])).expect("run status"),
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

pub fn runs(value: &Value) -> Vec<Run> {
    value.as_array().expect("runs").iter().map(run).collect()
}

pub fn select<'a>(pool: &'a [Run], ids: &Value) -> Vec<&'a Run> {
    strings(ids)
        .iter()
        .map(|id| {
            pool.iter()
                .find(|run| &run.id == id)
                .unwrap_or_else(|| panic!("undeclared run id {id}"))
        })
        .collect()
}

/// `{role, content}` vector messages.
pub fn messages(value: &Value) -> Vec<Message> {
    value
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| {
            let content = text(&message["content"]).to_owned();
            match text(&message["role"]) {
                "system" => Message::System { content },
                "user" => Message::User { content },
                other => panic!("unexpected role {other}"),
            }
        })
        .collect()
}

pub fn messages_json(messages: &[Message]) -> Value {
    json!(
        messages
            .iter()
            .map(|message| match message {
                Message::System { content } => json!({"role": "system", "content": content}),
                Message::User { content } => json!({"role": "user", "content": content}),
                other => panic!("unexpected message {other:?}"),
            })
            .collect::<Vec<_>>()
    )
}

fn effort(name: &str) -> Effort {
    match name {
        "off" => Effort::Off,
        "low" => Effort::Low,
        "medium" => Effort::Medium,
        "high" => Effort::High,
        other => panic!("unknown effort {other}"),
    }
}

/// v4's `reasoning_effort` setting: empty means unset.
pub fn reasoning(value: &Value) -> Option<Effort> {
    Some(text(value))
        .filter(|name| !name.is_empty())
        .map(effort)
}

/// The vectors' capability sets as v5 capabilities.
pub fn capabilities(raw: &Value) -> ModelCapabilities {
    let mut caps = ModelCapabilities::minimal();
    caps.structured_output = flag(&raw["structured_output"]);
    caps.sampling_controls = flag(&raw["sampling_controls"]);
    caps.reasoning_control = flag(&raw["reasoning_control"]);
    caps.reasoning_efforts = opt_strings(&raw["reasoning_efforts"])
        .map(|names| names.iter().map(|name| effort(name)).collect());
    caps
}

fn opt_strings(value: &Value) -> Option<Vec<String>> {
    (!value.is_null()).then(|| strings(value))
}

/// The wire body the runner would send, or the refusal it raises first.
pub fn body(request: &ChatRequest, caps: &ModelCapabilities) -> Value {
    match wire_body(request, caps) {
        Ok(body) => body,
        Err(error) => json!({ "refused": format!("{:?}", error.code) }),
    }
}
