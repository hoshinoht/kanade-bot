//! Replays `mentions.json`: levels, kinds, audiences and the allow-list gate.

use std::collections::BTreeMap;

use kanade::domain::{
    members::{Directory, PingError, PingLevel, normalise_level},
    notify::{
        PingKind, allowed_mentions, audience, everyone_on, not_declined, resolve_mentions,
        wants_mention,
    },
    schedule::RsvpState,
};
use serde_json::{Value, json};

use crate::{
    common::{self, strings, text},
    harness,
};

/// Candidates as v4 `str()`s them; the vectors include an integer id.
fn candidates(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("candidates")
        .iter()
        .map(|item| match item {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            other => panic!("unexpected candidate {other}"),
        })
        .collect()
}

fn kind(step: &Value) -> PingKind {
    PingKind::parse(text(&step["kind"]))
}

fn error_type(error: &PingError) -> &'static str {
    match error {
        PingError::InvalidLevel(_) => "ValueError",
        PingError::UnknownMember(_) => "KeyError",
    }
}

fn result(step: &Value, value: Result<Value, PingError>) -> Value {
    match (value, step.get("error_type")) {
        (Ok(value), None) => json!({ "value": value }),
        (Err(error), Some(declared)) => {
            assert_eq!(declared, error_type(&error), "{step}: error type");
            json!({ "error": { "type": declared, "message": error.to_string() } })
        }
        (Ok(value), Some(_)) => panic!("{step}: expected an error, got {value}"),
        (Err(error), None) => panic!("{step}: failed unexpectedly: {error}"),
    }
}

fn kind_names(kinds: &[PingKind]) -> Vec<&str> {
    let mut names: Vec<&str> = kinds.iter().map(PingKind::as_str).collect();
    names.sort_unstable();
    names
}

fn replay(case: &Value) -> Value {
    let input = &case["input"];
    let mut roster = harness::roster(input);
    let mut extra = Vec::new();
    let mut steps = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        let value: Result<Value, PingError> = match text(&step["op"]) {
            "ping_kinds" => Ok(json!({
                "essential": kind_names(PingKind::ESSENTIAL),
                "informational": kind_names(PingKind::INFORMATIONAL),
            })),
            "resolve_mentions" => Ok(json!(resolve_mentions(
                &roster,
                &candidates(&step["candidates"]),
                &kind(step),
            ))),
            "audience" => {
                let people = strings(&step["people"]);
                let wanted = (!step["candidates"].is_null()).then(|| strings(&step["candidates"]));
                let who = audience(&roster, &people, &kind(step), wanted.as_deref());
                Ok(json!({ "mentioned": who.mentioned, "names": who.names }))
            }
            "wants_mention" => {
                let level = PingLevel::parse_stored(text(&step["level"])).expect("level");
                Ok(json!(wants_mention(level, &kind(step))))
            }
            "normalise_level" => {
                normalise_level(step["value"].as_str()).map(|level| json!(level.as_str()))
            }
            "set_ping_level" => {
                let user_id = text(&step["user_id"]);
                if roster.get(user_id).is_none() && !extra.iter().any(|id| id == user_id) {
                    extra.push(user_id.to_owned());
                }
                roster
                    .set_ping_level(user_id, text(&step["level"]))
                    .map(|level| json!(level.as_str()))
            }
            "not_declined" => {
                let rsvps: BTreeMap<String, RsvpState> = step["rsvps"]
                    .as_array()
                    .expect("rsvps")
                    .iter()
                    .map(|pair| {
                        (
                            text(&pair[0]).to_owned(),
                            RsvpState::parse(text(&pair[1])).expect("rsvp state"),
                        )
                    })
                    .collect();
                Ok(json!(not_declined(&strings(&step["participants"]), &rsvps)))
            }
            "everyone_on" => {
                let runs: Vec<Vec<String>> = step["runs"]
                    .as_array()
                    .expect("runs")
                    .iter()
                    .map(strings)
                    .collect();
                Ok(json!(everyone_on(runs.iter().map(Vec::as_slice))))
            }
            "prepared_allow_list" => {
                let card = strings(&step["card_mentions"]);
                let users =
                    (!step["mention_users"].is_null()).then(|| strings(&step["mention_users"]));
                let quiet = step["quiet_mode"].as_bool().expect("quiet_mode");
                let allowed = allowed_mentions(&card, users.as_deref(), quiet);
                let users = if allowed.replied_user() {
                    json!(allowed.users())
                } else {
                    json!(false)
                };
                Ok(json!({
                    "users": users,
                    "roles": false,
                    "everyone": false,
                    "replied_user": allowed.replied_user(),
                }))
            }
            other => panic!("unknown mentions vector operation {other:?}"),
        };
        steps.push(result(step, value));
    }
    let known: Vec<String> = input["members"]
        .as_array()
        .expect("members")
        .iter()
        .map(|member| text(&member["user_id"]).to_owned())
        .collect();
    let members: Vec<Value> = known
        .iter()
        .chain(&extra)
        .map(|user_id| {
            json!({
                "user_id": user_id,
                "ping_level": roster.ping_level(user_id).as_str(),
                "known": roster.get(user_id).is_some(),
            })
        })
        .collect();
    json!({ "steps": steps, "final_state": { "members": members } })
}

#[test]
fn every_mentions_case_replays_exactly() {
    let file = common::load("mentions.json");
    assert_eq!(file["family"], "mentions");
    assert_eq!(file["schema_version"], "v5-scheduler-mentions-v1");
    let cases = file["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    let mut replayed = 0;
    for case in cases {
        let case_id = text(&case["case_id"]);
        assert_eq!(replay(case), case["expected"], "{case_id}");
        replayed += 1;
    }
    assert_eq!(replayed, cases.len(), "skipped cases");
}
