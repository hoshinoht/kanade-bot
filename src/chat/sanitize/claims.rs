//! False card claims and clarifications after the loop.

use std::sync::LazyLock;

use regex::Regex;

use super::{pattern_i, splitlines};
use crate::domain::pytext::strip;

/// A write claim that must never survive a refusal.
static WRITE_CLAIM: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"card'?s up|\bposted\b|✅"));

/// A new-card claim on a read-only turn; narrow so general ✅ advice survives.
static READ_FALSE_CLAIM: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(r"proposal card is ready|card (is|’s|'s) (ready|up|posted)|a card (has been )?posted")
});

/// Drop lines claiming a new card when none was posted.
pub fn strip_false_card_claim(text: &str) -> String {
    let kept: Vec<&str> = splitlines(text)
        .into_iter()
        .filter(|line| !READ_FALSE_CLAIM.is_match(line))
        .collect();
    let joined = kept.join("\n");
    let stripped = strip(&joined);
    if stripped.is_empty() {
        strip(text).to_owned()
    } else {
        stripped.to_owned()
    }
}

/// A refused-turn reply that already asks the member something (and claims
/// no card went up).
pub fn looks_like_clarification(text: &str) -> bool {
    text.contains('?') && !WRITE_CLAIM.is_match(text)
}
