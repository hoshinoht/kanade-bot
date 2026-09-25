//! Python `re` details the v4 patterns depend on.
//!
//! The `regex` crate's leftmost-first matching agrees with Python's backtracking
//! engine for these lookaround-free patterns, and its Unicode `\d` and `\b`
//! follow Python's `str` patterns closely enough (its `\w` also admits marks
//! and connector punctuation beyond `_`). Python's `\s` additionally matches
//! U+001C..U+001F, so patterns use [`SPACE`] instead.

use regex::Regex;

use crate::domain::pytext::decimal;

pub(super) const SPACE: &str = r"[\s\x1C-\x1F]";
pub(super) const NON_SPACE: &str = r"[^\s\x1C-\x1F]";

pub(super) fn pattern(source: &str) -> Regex {
    Regex::new(source).unwrap_or_else(|error| panic!("invalid pattern {source:?}: {error}"))
}

/// Python `int()` of a short run of decimal digits (any Unicode `Nd`).
pub(super) fn int(digits: &str) -> Option<u32> {
    digits.chars().try_fold(0u32, |total, c| {
        total.checked_mul(10)?.checked_add(decimal(c)?)
    })
}

/// Python `text[:-n]` and `text[-n:]` by code points.
pub(super) fn split_tail(text: &str, n: usize) -> (&str, &str) {
    let at = text
        .char_indices()
        .rev()
        .nth(n - 1)
        .map_or(0, |(index, _)| index);
    text.split_at(at)
}
