//! Keep listed schedule facts in the tool's canonical rendering (v4
//! `_ground_schedule_reply`) and the whole post-loop reply shaping.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::member::member_facing;
use super::tidy::{SCHEDULE_RUN_LINE, tidy};
use super::{pattern, pattern_i, splitlines};
use crate::chat::tools::{ToolName, ToolOutcome};
use crate::domain::pytext::strip;

static RECORD_ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"`?\[[0-9a-fA-F]{8}\]`?"));
static PRIMARY_LINE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^\s*(?:[-*]\s+)?\*\*.+ — .+\*\*$"));
static RUN_ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"\[([0-9a-fA-F]{8})\]"));
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\b(?:runs|boss week|schedule(?:d)?|all channels|this channel)\b"));
static TITLE: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\b(?:boss week|all channels|this channel)\b"));
static RUN_ID_WORD: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"\brun\s*ids?\b"));
static CHANNEL_DUMP: LazyLock<Regex> = LazyLock::new(|| pattern(r"\[#\d+\]|<#\d+>"));
static TIME: LazyLock<Regex> = LazyLock::new(|| pattern(r"\b\d{1,2}:\d{2}\b"));
static TALLY: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"\b\d+/\d+\s*(?:yes)?\b"));
static OMISSION: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"^\s*\*?\(and \d+ more\)\*?\s*$"));
static FOOTER: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"^\s*\*?Every run listed has already happened"));

/// The latest successful `get_schedule` listing with record ids, if any.
pub fn canonical_schedule_output(outcomes: &[ToolOutcome]) -> Option<&str> {
    outcomes
        .iter()
        .rev()
        .find(|outcome| {
            outcome.name == ToolName::GetSchedule.as_str()
                && outcome.ok
                && splitlines(&outcome.output)
                    .iter()
                    .any(|line| RECORD_ID.is_match(line))
        })
        .map(|outcome| outcome.output.as_str())
}

/// v4 `(?<![0-9a-f]){rid}(?![0-9a-f])` on the lowercased line.
fn mentions_id(line: &str, ids: &BTreeSet<String>) -> bool {
    let lowered = line.to_lowercase();
    let hex = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    ids.iter().any(|rid| {
        lowered.match_indices(rid.as_str()).any(|(at, _)| {
            !hex(lowered[..at].chars().next_back())
                && !hex(lowered[at + rid.len()..].chars().next())
        })
    })
}

fn has_schedule_facts(line: &str) -> bool {
    TIME.is_match(line) || TALLY.is_match(line) || CHANNEL_DUMP.is_match(line)
}

fn is_hint(line: &str) -> bool {
    TITLE.is_match(line)
        || RUN_ID_WORD.is_match(line)
        || CHANNEL_DUMP.is_match(line)
        || TIME.is_match(line)
        || TALLY.is_match(line)
}

/// Replace the model's retelling of the listing with the listing itself.
pub fn ground_schedule_reply(reply: &str, outcomes: &[ToolOutcome]) -> String {
    let Some(schedule) = canonical_schedule_output(outcomes) else {
        return reply.to_owned();
    };
    if strip(reply) == strip(schedule) {
        return schedule.to_owned();
    }
    let ids: BTreeSet<String> = RUN_ID
        .captures_iter(schedule)
        .map(|found| found[1].to_lowercase())
        .collect();
    let lines = splitlines(reply);
    let known = |line: &str| mentions_id(line, &ids);

    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let next = lines.get(index + 1).copied();
        // A record line with its facts line, or a `**day — boss**` line
        // whose facts line carries the id.
        let record = known(line)
            && SCHEDULE_RUN_LINE.is_match(line)
            && line.contains("**")
            && next.is_some_and(has_schedule_facts);
        let primary = PRIMARY_LINE.is_match(line)
            && next.is_some_and(|next| known(next) && has_schedule_facts(next));
        if record || primary {
            spans.push((index, index + 2));
            index += 2;
        } else if known(line) && has_schedule_facts(line) {
            spans.push((index, index + 1));
            index += 1;
        } else {
            index += 1;
        }
    }

    if !spans.is_empty() {
        let blank = |at: usize| strip(lines[at]).is_empty();
        let mut blocks: Vec<[usize; 2]> = Vec::new();
        for (start, end) in spans {
            match blocks.last_mut() {
                Some(block) if (block[1]..start).all(blank) => block[1] = end,
                _ => blocks.push([start, end]),
            }
        }
        for block in &mut blocks {
            let mut heading = block[0];
            while heading > 0 && blank(heading - 1) {
                heading -= 1;
            }
            if heading > 0 && HEADING.is_match(lines[heading - 1]) {
                block[0] = heading - 1;
            }
            let mut marker = block[1];
            while marker < lines.len() && blank(marker) {
                marker += 1;
            }
            if marker < lines.len()
                && (OMISSION.is_match(lines[marker]) || FOOTER.is_match(lines[marker]))
            {
                block[1] = marker + 1;
            }
        }
        let mut rebuilt: Vec<&str> = Vec::new();
        let mut cursor = 0;
        for (number, [start, end]) in blocks.iter().copied().enumerate() {
            rebuilt.extend_from_slice(&lines[cursor..start]);
            if number == 0 {
                rebuilt.extend(splitlines(schedule));
            }
            cursor = end;
        }
        rebuilt.extend_from_slice(&lines[cursor..]);
        return strip(&rebuilt.join("\n")).to_owned();
    }

    let hints: Vec<usize> = (0..lines.len()).filter(|&at| is_hint(lines[at])).collect();
    if let (Some(&first), Some(&last)) = (hints.first(), hints.last()) {
        let before = lines[..first].join("\n");
        let after = lines[last + 1..].join("\n");
        let parts: Vec<&str> = [strip(&before), strip(schedule), strip(&after)]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        return parts.join("\n\n");
    }
    schedule.to_owned()
}

/// v4 `generate`'s reply shaping after the loop: reground, scrub, bound.
pub fn shape_reply(reply: &str, outcomes: &[ToolOutcome]) -> String {
    let grounded = ground_schedule_reply(reply, outcomes);
    let protected =
        canonical_schedule_output(outcomes).filter(|schedule| grounded.contains(schedule));
    tidy(&member_facing(&grounded), protected)
}
