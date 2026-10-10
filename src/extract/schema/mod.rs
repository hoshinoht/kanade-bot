//! The structured output the extractor is constrained to (v4
//! `bot/extract/schema.py` and `parse_response`).
//!
//! The strict schema narrows "did it return JSON", not "did it return sensible
//! JSON", so every field is still validated and coerced where a small model
//! predictably slips; v4's coercions are kept, oddities included.

mod call;
mod coerce;
mod parse;
mod pyvalue;
mod text;

use serde_json::Value;

pub use call::{AttemptOutcome, ExtractionAttempts, ExtractionCall, Next, retry_instruction};
pub use coerce::{FieldError, Loc};
pub use parse::{ParseError, parse_response};

use super::Amendment;

/// Everything the model found in one burst of messages.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extraction {
    pub amendments: Vec<Amendment>,
    pub summary: String,
}

/// The schema text exactly as v4 embeds it in a prompt.
pub fn schema_text() -> &'static str {
    text::SCHEMA_TEXT
}

/// The strict schema sent as the extractor's structured output.
pub fn extraction_schema() -> Value {
    serde_json::from_str(text::SCHEMA_TEXT).expect("schema text is JSON")
}
