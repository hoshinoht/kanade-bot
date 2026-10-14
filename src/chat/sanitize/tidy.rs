//! Normalise and bound a member-facing reply (v4 `ChatPilot._tidy`).

use std::sync::LazyLock;

use regex::Regex;

use super::fence::map_prose;
use super::pattern;
use crate::chat::tools::MAX_MEMBER_REPLY;
use crate::domain::pytext::strip;

/// A list marker glued to its heading.
const GLUED_BULLET: &str = ": - ";

static BLANK_RUNS: LazyLock<Regex> = LazyLock::new(|| pattern(r"\n(?:[ \t]*\n){1,}"));
pub(super) static SCHEDULE_RUN_LINE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^\s*(?:[-*]\s*)?`?\[[0-9a-fA-F]{8}\]`?\s+\S"));

/// Collapse blank-line runs without splitting schedule records.
fn tidy_blank_lines(text: &str) -> String {
    let normalised = BLANK_RUNS.replace_all(text, "\n\n");
    let lines: Vec<&str> = normalised.split('\n').collect();
    let mut compact: Vec<&str> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let between_runs = line.is_empty()
            && compact
                .last()
                .is_some_and(|previous| SCHEDULE_RUN_LINE.is_match(previous))
            && lines
                .get(index + 1)
                .is_some_and(|next| SCHEDULE_RUN_LINE.is_match(next));
        if !between_runs {
            compact.push(line);
        }
    }
    compact.join("\n")
}

/// Repair a first list item glued to its heading.
pub fn unglue_first_bullet(text: &str) -> String {
    if text.contains("\n- ") {
        text.replace(GLUED_BULLET, ":\n\n- ")
    } else {
        text.to_owned()
    }
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

/// Normalise outside fenced code, then bound to [`MAX_MEMBER_REPLY`]; a
/// `protected` block (the canonical schedule listing) is kept whole and
/// whatever surrounds it is dropped first.
pub fn tidy(content: &str, protected: Option<&str>) -> String {
    bound(&normalise(content), protected)
}

/// [`tidy`] when its bound drops nothing.
pub(super) fn tidy_whole(content: &str, protected: Option<&str>) -> Option<String> {
    let text = normalise(content);
    let whole = match around(&text, protected) {
        Some((prefix, protected, suffix)) => [prefix, protected, suffix]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        None => strip(&text).to_owned(),
    };
    let bounded = bound(&text, protected);
    (bounded == whole).then_some(bounded)
}

/// [`tidy`] without the bound.
pub(super) fn normalise(content: &str) -> String {
    let text = map_prose(content, tidy_blank_lines);
    let text = strip(&text);
    if text.contains("\n- ") {
        map_prose(text, |prose| prose.replace(GLUED_BULLET, ":\n\n- "))
    } else {
        text.to_owned()
    }
}

/// The stripped text before and after a contained `protected` block.
fn around<'a>(text: &'a str, protected: Option<&'a str>) -> Option<(&'a str, &'a str, &'a str)> {
    let protected = protected.filter(|p| !p.is_empty() && text.contains(*p))?;
    let (before, after) = text.split_once(protected).expect("contains");
    Some((strip(before), protected, strip(after)))
}

fn bound(text: &str, protected: Option<&str>) -> String {
    if let Some((prefix, protected, suffix)) = around(text, protected) {
        let mut parts = vec![protected];
        if !prefix.is_empty() && chars(prefix) + chars(protected) + 2 <= MAX_MEMBER_REPLY {
            parts.insert(0, prefix);
        }
        if !suffix.is_empty() && chars(&parts.join("\n\n")) + chars(suffix) + 2 <= MAX_MEMBER_REPLY
        {
            parts.push(suffix);
        }
        return parts.join("\n\n");
    }
    let bounded: String = text.chars().take(MAX_MEMBER_REPLY).collect();
    strip(&bounded).to_owned()
}
