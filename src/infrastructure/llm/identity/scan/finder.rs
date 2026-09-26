use std::collections::HashSet;

use serde_json::Value;

use super::super::codec::ScanNeedles;
use super::super::pseudonym::matcher::{digit_run, fold, match_at, prev_char};
use super::{LeakFound, LeakKind, ScanExemptions};

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

    /// Every key and string (unescaped) and every number of a serialized
    /// request, except the top-level `model` alias (route config).
    pub(super) fn request(&self, request: &Value, found: &mut LeakFound) {
        let mut stack: Vec<&Value> = Vec::new();
        match request {
            Value::Object(map) => {
                for (key, item) in map {
                    self.text(key, found);
                    if key != "model" {
                        stack.push(item);
                    }
                }
            }
            other => stack.push(other),
        }
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
