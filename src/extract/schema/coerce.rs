//! v4's pydantic `Extraction`/`Amendment` validation in lax mode, including
//! its `mode="before"` coercions. Errors carry pydantic's `loc`/`type` pairs.

use std::fmt;

use serde_json::{Map, Value};

use super::Extraction;
use super::pyvalue::py_str;
use crate::domain::pytext::strip;
use crate::domain::schedule::RsvpState;
use crate::extract::{Amendment, AmendmentKind};

/// Strings a small model reaches for when it means "nothing here".
const NULLISH: [&str; 9] = ["", "null", "none", "n/a", "na", "unknown", "tbd", "-", "?"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loc {
    Field(&'static str),
    Index(usize),
}

impl fmt::Display for Loc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Field(name) => f.write_str(name),
            Self::Index(index) => write!(f, "{index}"),
        }
    }
}

/// One pydantic error: where, and its error `type`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub loc: Vec<Loc>,
    pub kind: &'static str,
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let loc: Vec<String> = self.loc.iter().map(ToString::to_string).collect();
        write!(f, "{}\n  [type={}]", loc.join("."), self.kind)
    }
}

/// `_clean`: text, or `None` for null and the nullish words.
fn clean(value: &Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let text = strip(&py_str(value)).to_owned();
    (!NULLISH.contains(&text.to_lowercase().as_str())).then_some(text)
}

/// `_as_list`: a list, a bare or comma/semicolon separated string, or one value;
/// mentions lose their `<@!>` wrapping and duplicates are dropped.
fn as_list(value: Option<&Value>) -> Vec<String> {
    let items: Vec<Value> = match value {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::String(text)) => text
            .replace(';', ",")
            .split(',')
            .map(|part| Value::String(part.to_owned()))
            .collect(),
        Some(Value::Array(items)) => items.clone(),
        Some(other) => vec![other.clone()],
    };
    let mut out: Vec<String> = Vec::new();
    for item in &items {
        let Some(text) = clean(item) else { continue };
        let text = text.trim_matches(['<', '>', '@', '!']);
        if !text.is_empty() && !out.iter().any(|seen| seen == text) {
            out.push(text.to_owned());
        }
    }
    out
}

/// Python `float(value)`, or `None` where it raises.
fn py_float(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        Value::String(text) => strip(text).parse().ok(),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// `_as_confidence`: nonsense is 0, a percentage is divided by 100, then clamped.
fn confidence(value: &Value) -> Result<f64, &'static str> {
    let Some(mut number) = py_float(value) else {
        return Ok(0.0);
    };
    if number > 1.0 {
        number /= 100.0;
    }
    if number.is_nan() {
        // Python's min/max keep NaN, which then fails `ge=0`.
        return Err("greater_than_equal");
    }
    Ok(number.clamp(0.0, 1.0))
}

fn is_question(value: &Value) -> Result<bool, &'static str> {
    match value {
        Value::String(text) => Ok(matches!(
            strip(text).to_lowercase().as_str(),
            "true" | "yes" | "1" | "y"
        )),
        Value::Bool(flag) => Ok(*flag),
        Value::Number(number) => match number.as_f64() {
            Some(0.0) => Ok(false),
            Some(1.0) => Ok(true),
            _ => Err("bool_parsing"),
        },
        Value::Null | Value::Array(_) | Value::Object(_) => Err("bool_type"),
    }
}

fn kind(value: Option<&Value>) -> Result<AmendmentKind, &'static str> {
    let value = value.ok_or("missing")?;
    clean(value)
        .and_then(|text| AmendmentKind::parse(&text.to_lowercase()))
        .ok_or("literal_error")
}

fn rsvp(value: Option<&Value>) -> Result<Option<RsvpState>, &'static str> {
    match value.and_then(clean) {
        None => Ok(None),
        Some(text) => match text.to_lowercase().as_str() {
            "yes" => Ok(Some(RsvpState::Yes)),
            "no" => Ok(Some(RsvpState::No)),
            "maybe" => Ok(Some(RsvpState::Maybe)),
            _ => Err("literal_error"),
        },
    }
}

fn amendment(
    map: &Map<String, Value>,
    at: &[Loc],
    errors: &mut Vec<FieldError>,
) -> Option<Amendment> {
    let before = errors.len();
    let mut fail = |field: &'static str, kind: &'static str| {
        let mut loc = at.to_vec();
        loc.push(Loc::Field(field));
        errors.push(FieldError { loc, kind });
    };
    let get = |field: &str| map.get(field);
    let text = |field: &str| get(field).and_then(clean);

    let kind = kind(get("kind")).map_err(|kind| fail("kind", kind)).ok();
    let rsvp = rsvp(get("rsvp")).map_err(|kind| fail("rsvp", kind)).ok();
    let is_question = match get("is_question") {
        None => Some(false),
        Some(value) => is_question(value)
            .map_err(|kind| fail("is_question", kind))
            .ok(),
    };
    let confidence = match get("confidence") {
        None => Some(0.0),
        Some(value) => confidence(value)
            .map_err(|kind| fail("confidence", kind))
            .ok(),
    };
    if errors.len() > before {
        return None;
    }
    Some(Amendment {
        kind: kind?,
        bosses: as_list(get("bosses")),
        day_ref: text("day_ref"),
        time_ref: text("time_ref"),
        participants: as_list(get("participants")),
        rsvp: rsvp?,
        is_question: is_question?,
        confidence: confidence?,
        evidence_message_ids: as_list(get("evidence_message_ids")),
        target_run_hint: text("target_run_hint"),
    })
}

/// Validate a decoded JSON object as v4's `Extraction.model_validate`.
pub(super) fn extraction(map: &Map<String, Value>) -> Result<Extraction, Vec<FieldError>> {
    let mut errors = Vec::new();
    let items: Vec<&Value> = match map.get("amendments") {
        None | Some(Value::Null) => Vec::new(),
        Some(object @ Value::Object(_)) => vec![object],
        Some(Value::Array(items)) => items.iter().collect(),
        Some(_) => {
            errors.push(FieldError {
                loc: vec![Loc::Field("amendments")],
                kind: "list_type",
            });
            Vec::new()
        }
    };
    let mut amendments = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let at = [Loc::Field("amendments"), Loc::Index(index)];
        match item {
            Value::Object(fields) => amendments.extend(amendment(fields, &at, &mut errors)),
            _ => errors.push(FieldError {
                loc: at.to_vec(),
                kind: "model_type",
            }),
        }
    }
    let summary = map.get("summary").and_then(clean).unwrap_or_default();
    if errors.is_empty() {
        Ok(Extraction {
            amendments,
            summary,
        })
    } else {
        Err(errors)
    }
}
