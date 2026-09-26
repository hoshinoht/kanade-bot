use std::collections::HashSet;

use serde_json::Value;

use super::super::codec::ScanNeedles;
use super::super::pseudonym::matcher::{digit_run, fold, match_at, prev_char};
use super::{LeakFound, LeakKind, ScanExemptions};
use crate::infrastructure::llm::{ChatRequest, Message};

const SNOWFLAKE_DIGITS: std::ops::RangeInclusive<usize> = 17..=20;

/// One scan's needles: names (longest first) minus code-owned words, ids,
/// and issued tokens (lowercase), which may legitimately equal a name.
pub(super) struct Finder {
    names: Vec<Vec<char>>,
    ids: HashSet<String>,
    tokens: HashSet<String>,
}

impl Finder {
    pub(super) fn new(needles: &ScanNeedles, exemptions: &ScanExemptions) -> Self {
        let mut names: Vec<Vec<char>> = needles
            .names
            .iter()
            .filter(|name| !exemptions.covers(&name.text))
            .map(|name| fold(&name.text))
            .filter(|folded| !folded.is_empty())
            .collect();
        names.sort_by_key(|folded| std::cmp::Reverse(folded.len()));
        names.dedup();
        Self {
            names,
            ids: needles.ids.iter().cloned().collect(),
            tokens: needles.tokens.iter().map(|t| lower(t)).collect(),
        }
    }

    /// The content-bearing fields of a request: message text, tool-call
    /// names and arguments, tool results, and tool definitions and output
    /// schemas (every key, string and number). Fixed structure (the model
    /// alias, roles, token limits, reasoning, sampling, `strict`, validation)
    /// is never scanned. Model-issued call ids get known-id matching only;
    /// an assistant turn equal to something this model wrote (`echoes`) is
    /// skipped.
    pub(super) fn request(
        &self,
        request: &ChatRequest,
        echoes: &HashSet<String>,
        found: &mut LeakFound,
    ) {
        for message in &request.messages {
            match message {
                Message::System { content } | Message::User { content } => {
                    self.text(content, found);
                }
                // The model's own words, repeated verbatim, are not scanned.
                Message::Assistant {
                    content,
                    tool_calls,
                } => {
                    if let Some(content) = content.as_ref().filter(|text| !echoes.contains(*text)) {
                        self.text(content, found);
                    }
                    for call in tool_calls {
                        self.call_id(&call.id, found);
                        self.text(&call.name, found);
                        if !echoes.contains(&call.arguments) {
                            self.embedded(&call.arguments, found);
                        }
                    }
                }
                Message::Tool {
                    tool_call_id,
                    content,
                } => {
                    self.call_id(tool_call_id, found);
                    self.embedded(content, found);
                }
            }
        }
        for tool in &request.tools {
            self.text(&tool.name, found);
            if let Some(description) = &tool.description {
                self.text(description, found);
            }
            self.value(&tool.input_schema, found);
        }
        if let Some(schema) = &request.output_schema {
            self.text(&schema.name, found);
            self.value(&schema.schema, found);
        }
    }

    /// Text that may itself be JSON (tool arguments and results): a JSON
    /// object or array is scanned structurally (every unescaped key and
    /// string, every number), so escapes such as `\u0041lice` cannot hide a
    /// name; anything else as text.
    fn embedded(&self, text: &str, found: &mut LeakFound) {
        let head = text.trim_start();
        if (head.starts_with('{') || head.starts_with('['))
            && let Ok(value) = serde_json::from_str::<Value>(text)
        {
            self.value(&value, found);
        } else {
            self.text(text, found);
        }
    }

    /// A provider-issued call id: a known member id is a leak, a long digit
    /// run alone is not (ids like `call_123…` are opaque).
    fn call_id(&self, id: &str, found: &mut LeakFound) {
        let mut i = 0;
        while i < id.len() {
            let rest = &id[i..];
            let prev = id[..i].chars().next_back();
            if prev.is_none_or(|ch| !ch.is_ascii_digit()) {
                let digits = digit_run(rest);
                if !digits.is_empty() {
                    if self.ids.contains(digits) {
                        found.add(LeakKind::Id);
                    }
                    i += digits.len();
                    continue;
                }
            }
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }

    /// Every key, string and number of a JSON value.
    fn value(&self, value: &Value, found: &mut LeakFound) {
        let mut stack = vec![value];
        while let Some(value) = stack.pop() {
            match value {
                Value::String(text) => self.text(text, found),
                Value::Number(number) => self.text(&number.to_string(), found),
                Value::Array(items) => stack.extend(items),
                Value::Object(map) => {
                    for (key, item) in map {
                        self.text(key, found);
                        stack.push(item);
                    }
                }
                Value::Null | Value::Bool(_) => {}
            }
        }
    }

    /// Codec matching rules: whole-word, Unicode lowercase-insensitive names
    /// (longest first, a possible backslash escape counts as a boundary) and
    /// maximal digit runs equal to a known id or 17–20 digits long.
    pub(super) fn text(&self, text: &str, found: &mut LeakFound) {
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            let prev = prev_char(text, i);
            if let Some(len) = self.name_at(prev, rest) {
                // An issued token spelled exactly like a masked name (an
                // author registered after the token was issued) is not a leak.
                if !self.tokens.contains(&lower(&rest[..len])) {
                    found.add(LeakKind::Name);
                }
                i += len;
                continue;
            }
            if prev.is_none_or(|ch| !ch.is_ascii_digit()) {
                let digits = digit_run(rest);
                if !digits.is_empty() {
                    if self.ids.contains(digits) {
                        found.add(LeakKind::Id);
                    } else if SNOWFLAKE_DIGITS.contains(&digits.len()) {
                        found.add(LeakKind::Snowflake);
                    }
                    i += digits.len();
                    continue;
                }
            }
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }

    fn name_at(&self, prev: Option<char>, rest: &str) -> Option<usize> {
        let first = rest.chars().next()?.to_lowercase().next()?;
        self.names.iter().find_map(|folded| {
            if folded.first() != Some(&first) {
                return None;
            }
            match_at(prev, rest, folded)
        })
    }
}

fn lower(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}
