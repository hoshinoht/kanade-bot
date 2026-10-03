//! `D-PERSONAL-CONTEXT`: the model-only context line under a personal
//! `get_schedule` record is split off before grounding, so the canonical
//! listing members see never carries it, and a copy of it in a reply is
//! removed. Its exact phrases are what a line citing that run may voice.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::{pattern, pattern_i};
use crate::chat::tools::read::format::CONTEXT_LABEL;
use crate::chat::tools::{ToolName, ToolOutcome};
use crate::domain::pytext::strip;

static RUN_ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"\[([0-9a-fA-F]{8})\]"));
/// The label however a reply dresses it: any case, markup, brackets or
/// separator, with or without the leading `Context`.
static LABEL: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"(?:\bcontext\b[^\w\n]{0,8})?\bhidden\s+from\s+members\b"));
/// One phrase of the context vocabulary (`format::run_context`).
const PHRASE: &str = r"(?:today|tonight|tomorrow|in\s+[0-9]+\s+(?:days?|hours?|minutes?)|you\s+said\s+(?:yes|no|maybe)|you\s+haven['’]?t\s+answered|no\s+answer\s+yet\s+from\s+[^·•|\n]+?)";
/// Two or more vocabulary phrases joined as the context line joins them.
static JOINED: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(&format!(r"{PHRASE}\s*[·•|]\s*{PHRASE}")));
/// A line that is only vocabulary phrases.
static ONLY_PHRASES: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(&format!(r"^{PHRASE}(?:\s*[·•|;]\s*{PHRASE})*[.!~]*$")));
/// A bare `Context:` heading over copied phrases.
static HEADING: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"^context\s*:?$"));
const SEPARATOR: &str = " · ";

/// Each listed run's context phrases, by lowercase short id.
pub(super) type Contexts = BTreeMap<String, Vec<String>>;

/// The listing without its context lines, and each run's phrases.
pub(super) fn split(raw: &str) -> (String, Contexts) {
    let mut contexts = Contexts::new();
    if !raw.contains(CONTEXT_LABEL) {
        return (raw.to_owned(), contexts);
    }
    let paragraphs: Vec<String> = raw
        .split("\n\n")
        .map(|paragraph| {
            let id = RUN_ID
                .captures(paragraph)
                .map(|found| found[1].to_lowercase());
            let mut kept: Vec<&str> = Vec::new();
            for line in paragraph.split('\n') {
                match (line.strip_prefix(CONTEXT_LABEL), &id) {
                    (Some(body), Some(id)) => {
                        contexts
                            .entry(id.clone())
                            .or_default()
                            .extend(body.split(SEPARATOR).map(str::to_owned));
                    }
                    (Some(_), None) => {}
                    (None, _) => kept.push(line),
                }
            }
            kept.join("\n")
        })
        .collect();
    (paragraphs.join("\n\n"), contexts)
}

/// Every multi-phrase context body any successful `get_schedule` call
/// returned; a one-phrase body (`tonight`) is ordinary voicing.
fn bodies(outcomes: &[ToolOutcome]) -> Vec<String> {
    outcomes
        .iter()
        .filter(|outcome| outcome.ok && outcome.name == ToolName::GetSchedule.as_str())
        .flat_map(|outcome| outcome.output.lines())
        .filter_map(|line| line.strip_prefix(CONTEXT_LABEL))
        .filter(|body| body.contains(SEPARATOR))
        .map(|body| body.trim().to_lowercase())
        .collect()
}

/// Markup a copied line may be wrapped in.
fn bare(line: &str) -> &str {
    strip(line).trim_matches(|c: char| c.is_whitespace() || "*_`>-•~".contains(c))
}

/// The reply without copied context (`D-PERSONAL-CONTEXT`): every line
/// carrying the label, however dressed; every line holding only context
/// phrases (bullets included) or a bare `Context:` heading; any line holding
/// two or more phrases joined as the context line joins them; and any line
/// that is a copied multi-phrase body. Code fences are not exempt. Works
/// without outcomes (a card-posting turn) from the phrase vocabulary alone.
pub fn strip_context_copies(reply: &str, outcomes: &[ToolOutcome]) -> String {
    let bodies = bodies(outcomes);
    let copied = |line: &str| {
        let plain = bare(line);
        LABEL.is_match(line)
            || JOINED.is_match(line)
            || (!plain.is_empty() && ONLY_PHRASES.is_match(plain))
            || HEADING.is_match(plain)
            || bodies.contains(&plain.to_lowercase())
    };
    if !reply.split('\n').any(copied) {
        return reply.to_owned();
    }
    reply
        .split('\n')
        .filter(|line| !copied(line))
        .collect::<Vec<_>>()
        .join("\n")
}
