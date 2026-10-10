use std::sync::LazyLock;
use std::time::Duration;

use kanade::extract::prompt::extraction_request;
use kanade::extract::schema::{
    AttemptOutcome, ExtractionAttempts, Loc, Next, ParseError, parse_response,
};
use regex::Regex;
use serde_json::{Value, json};

use crate::shaping;
use crate::support::{
    Deviation, Outcome, body, capabilities, extraction_json, messages, opt_text, reasoning,
    replay_family_with, text, unknown_op,
};

fn parsed(raw: &str) -> Value {
    match parse_response(raw) {
        Ok(extraction) => {
            json!({ "status": "accepted", "extraction": extraction_json(&extraction) })
        }
        Err(ParseError::Invalid(errors)) => json!({
            "status": "rejected",
            "error_class": "ValidationError",
            "errors": errors.iter().map(|error| json!({
                "loc": error.loc.iter().map(|loc| match loc {
                    Loc::Field(name) => json!(name),
                    Loc::Index(index) => json!(index),
                }).collect::<Vec<_>>(),
                "type": error.kind,
            })).collect::<Vec<_>>(),
        }),
        Err(error) => json!({
            "status": "rejected",
            "error_class": error.error_class(),
            "message": error.to_string(),
        }),
    }
}

/// Drives the pure retry decision the way E4 must: build the request from the
/// current conversation, send it (here: a scripted reply), report the outcome.
/// Transport failures arrive already rendered as `Kind: message`.
fn extract_call(step: &Value) -> Value {
    let model = text(&step["extract_model"]);
    let caps = capabilities(&step["caps"]);
    let limit = Duration::from_secs_f64(step["kanata_timeout"].as_f64().expect("timeout"));
    let mut replies = step["replies"].as_array().expect("replies").iter();
    let mut attempts = ExtractionAttempts::new(messages(&step["messages"]));
    let mut requests = Vec::new();
    let call = loop {
        let outcome = if model.is_empty() {
            AttemptOutcome::Misconfigured {
                detail: "ModelConfigError: EXTRACT_MODEL is not set".into(),
            }
        } else {
            let request = extraction_request(
                model,
                attempts.messages().to_vec(),
                reasoning(&step["reasoning_effort"]),
            );
            requests.push(body(&request, &caps));
            let reply = replies.next().expect("scripted reply");
            match reply.get("raise").map(text) {
                None => AttemptOutcome::Reply {
                    content: opt_text(&reply["content"]).map(str::to_owned),
                    reasoning: opt_text(&reply["reasoning"]).map(str::to_owned),
                },
                Some("TimeoutError") => AttemptOutcome::TimedOut { limit },
                Some(kind) => AttemptOutcome::Failed {
                    detail: format!("{kind}: {}", text(&reply["message"])),
                },
            }
        };
        match attempts.record(outcome) {
            Next::Retry => continue,
            Next::Done(call) => break call,
        }
    };
    assert!(replies.next().is_none(), "scripted replies unused");
    json!({
        "ok": call.ok(),
        "attempts": call.attempts,
        "error": call.error,
        "raw": call.raw,
        "thinking": call.thinking,
        "misconfigured": call.misconfigured,
        "prompt": call.prompt,
        "extraction": call.extraction.as_ref().map(extraction_json),
        "requests": requests,
    })
}

fn replay(_input: &Value, step: &Value) -> Outcome {
    let value = match text(&step["op"]) {
        "parse_response" => parsed(text(&step["raw"])),
        "extract_call" => extract_call(step),
        other => unknown_op("parse", other),
    };
    Ok(value)
}

static VALIDATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^\d+ validation errors? for Extraction.*$").unwrap());

/// The README's non-portable text: the JSON decoder's detail after
/// `not JSON: ` and a validation error's prose. Envelopes still compare.
fn portable_error(error: &str) -> String {
    if error.starts_with("not JSON: ") {
        return "not JSON: <decoder detail>".to_owned();
    }
    VALIDATION.replace(error, "<validation error>").into_owned()
}

const RETRY_PREFIX: &str = "Your previous answer did not fit the schema:\n";
const RETRY_SUFFIX: &str = "\nAnswer again with the same information in the required shape. \
                            Do not add anything the messages do not say.";

fn portable(op: &str, value: &mut Value) {
    match op {
        "parse_response" => {
            if let Some(message) = value.get("message").and_then(Value::as_str) {
                value["message"] = json!(portable_error(message));
            }
        }
        "extract_call" => {
            if let Some(error) = value["error"].as_str() {
                value["error"] = json!(portable_error(error));
            }
            for request in value["requests"].as_array_mut().expect("requests") {
                let Some(messages) = request.get_mut("messages").and_then(Value::as_array_mut)
                else {
                    continue;
                };
                for message in messages {
                    let content = message["content"].as_str().unwrap_or_default();
                    if let Some(error) = content
                        .strip_prefix(RETRY_PREFIX)
                        .and_then(|rest| rest.strip_suffix(RETRY_SUFFIX))
                    {
                        message["content"] = json!(format!(
                            "{RETRY_PREFIX}{}{RETRY_SUFFIX}",
                            portable_error(error)
                        ));
                    }
                }
            }
        }
        _ => {}
    }
}

const fn requests(case_id: &'static str, step: usize) -> Deviation {
    Deviation {
        name: "D-SHAPING extraction requests through the runner",
        case_id,
        step,
        rewrite: shaping::requests,
    }
}

// `guarded-call-transport-failures` step 4 (no model) sends nothing to shape.
const DEVIATIONS: [Deviation; 9] = [
    requests("guarded-call-accepts-first-answer", 0),
    requests("guarded-call-accepts-first-answer", 1),
    requests("guarded-call-retries-once-then-quarantines", 0),
    requests("guarded-call-retries-once-then-quarantines", 1),
    requests("guarded-call-retries-once-then-quarantines", 2),
    requests("guarded-call-transport-failures", 0),
    requests("guarded-call-transport-failures", 1),
    requests("guarded-call-transport-failures", 2),
    requests("guarded-call-transport-failures", 3),
];

#[test]
fn parse_vectors_replay_exactly() {
    let replayed = replay_family_with("parse", &DEVIATIONS, portable, replay);
    assert_eq!(replayed, (5, 30));
}
