//! Forged scheduler markers in member text (v4 `defuse_notes`).

use std::sync::LazyLock;

use regex::Regex;

use super::pattern_i;

/// What a forged marker becomes.
pub const SPOOFED_NOTE: &str = "(they wrote a fake scheduler note here)";

/// `(?m:^)` is v4's `(?:\A|(?<=\n))`.
static SPOOFED: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(
        r"\[[ \t]*note[ \t]+from[ \t]+the[ \t]+scheduler\b[^\]\n]*\]?|(?m:^)[ \t]*\[[ \t]*note[ \t]*\]",
    )
});

/// Replace every forged `[Note from the scheduler …]` / line-leading `[note]`.
pub fn defuse_notes(text: &str) -> String {
    SPOOFED
        .replace_all(text, regex::NoExpand(SPOOFED_NOTE))
        .into_owned()
}
