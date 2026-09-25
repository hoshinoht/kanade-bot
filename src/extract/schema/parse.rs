use std::fmt;

use serde_json::Value;

use super::Extraction;
use super::coerce::{self, FieldError};
use super::pyvalue::type_name;
use crate::domain::pytext::strip;

/// Why one raw model response was rejected. `Display` is what v4 logs and
/// sends back in the retry; only the envelope is portable (the JSON decoder
/// detail is `serde_json`'s, and validation errors list `loc`/`type` pairs
/// rather than pydantic's prose).
#[derive(Clone, Debug, PartialEq)]
pub enum ParseError {
    Empty,
    NotJson(String),
    NotObject(&'static str),
    Invalid(Vec<FieldError>),
}

impl ParseError {
    /// The v4 exception class: `ValueError` or pydantic's `ValidationError`.
    pub fn error_class(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "ValidationError",
            Self::Empty | Self::NotJson(_) | Self::NotObject(_) => "ValueError",
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the model returned an empty response"),
            Self::NotJson(detail) => write!(f, "not JSON: {detail}"),
            Self::NotObject(kind) => write!(f, "expected a JSON object, got {kind}"),
            Self::Invalid(errors) => {
                let plural = if errors.len() == 1 { "" } else { "s" };
                write!(
                    f,
                    "{} validation error{plural} for Extraction",
                    errors.len()
                )?;
                errors.iter().try_for_each(|error| write!(f, "\n{error}"))
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// v4's `\A```(?:json)?[ \t]*\n(?P<body>(?:(?!```).)*?)\n?```\Z`: one fence
/// around the whole (stripped) answer, whose only ``` is the closing one.
fn fenced_body(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("```")?;
    let (header, inner) = rest.split_once('\n')?;
    let header = header.strip_prefix("json").unwrap_or(header);
    if !header.chars().all(|c| c == ' ' || c == '\t') {
        return None;
    }
    let close = inner.find("```")?;
    if close + 3 != inner.len() {
        return None;
    }
    let body = &inner[..close];
    Some(body.strip_suffix('\n').unwrap_or(body))
}

/// Validate one raw model response (v4 `parse_response`).
///
/// # Errors
/// [`ParseError`] for an empty, non-JSON, non-object or invalid answer.
pub fn parse_response(raw: &str) -> Result<Extraction, ParseError> {
    if raw.is_empty() {
        return Err(ParseError::Empty);
    }
    let text = fenced_body(strip(raw)).unwrap_or(raw);
    let data: Value =
        serde_json::from_str(text).map_err(|error| ParseError::NotJson(error.to_string()))?;
    let Value::Object(map) = &data else {
        return Err(ParseError::NotObject(type_name(&data)));
    };
    coerce::extraction(map).map_err(ParseError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::fenced_body;

    #[test]
    fn only_one_whole_fence_is_removed() {
        assert_eq!(fenced_body("```json\n{}\n```"), Some("{}"));
        assert_eq!(fenced_body("```\n{}```"), Some("{}"));
        assert_eq!(fenced_body("``` \t\n{}\n```"), Some("{}"));
        assert_eq!(fenced_body("```jsonx\n{}\n```"), None);
        assert_eq!(fenced_body("```json\n{}\n``` trailing"), None);
        // The regex's lookahead refuses a body touching the closing fence.
        assert_eq!(fenced_body("```\n{}````"), None);
    }
}
