//! Keep listed schedule facts in the tool's canonical rendering (v4
//! `_ground_schedule_reply`).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::fence::fenced;
use super::listing::Block;
use super::tidy::SCHEDULE_RUN_LINE;
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
fn names(lowered: &str, rid: &str) -> bool {
    let hex = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    lowered.match_indices(rid).any(|(at, _)| {
        !hex(lowered[..at].chars().next_back()) && !hex(lowered[at + rid.len()..].chars().next())
    })
}

fn mentions_id(line: &str, ids: &BTreeSet<String>) -> bool {
    let lowered = line.to_lowercase();
    ids.iter().any(|rid| names(&lowered, rid))
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

/// What grounding made of a reply, and the schedule text it put in.
pub(super) struct Grounded {
    pub text: String,
    pub block: Option<Block>,
}

/// Replace the model's retelling of the listing with the listing itself.
pub fn ground_schedule_reply(reply: &str, outcomes: &[ToolOutcome]) -> String {
    ground(reply, outcomes).text
}

/// Whether each line overlaps a paired code fence.
fn fenced_lines(text: &str, lines: &[&str]) -> Vec<bool> {
    let blocks = fenced(text);
    lines
        .iter()
        .map(|line| {
            let start = line.as_ptr() as usize - text.as_ptr() as usize;
            let end = start + line.len().max(1);
            blocks
                .iter()
                .any(|block| start < block.end && block.start < end)
        })
        .collect()
}

/// The listing's ids a line names.
fn named_ids<'a>(line: &str, ids: &'a BTreeSet<String>) -> impl Iterator<Item = String> + 'a {
    let lowered = line.to_lowercase();
    ids.iter().filter(move |rid| names(&lowered, rid)).cloned()
}

/// A record-shaped line whose bracketed id no tool output carries.
fn invented(line: &str, outcomes: &[ToolOutcome]) -> bool {
    SCHEDULE_RUN_LINE.is_match(line)
        && RUN_ID.captures(line).is_some_and(|found| {
            let rid = found[1].to_lowercase();
            !outcomes
                .iter()
                .any(|outcome| outcome.ok && outcome.output.to_lowercase().contains(&rid))
        })
}

/// v4 `_ground_schedule_reply` with the `D-GROUND-FILTERED` differences:
/// fenced code is never read as schedule text or replaced, invented record
/// lines go (at the first real run's place) when real runs are named, a
/// reply naming only some listed runs gets just their canonical records
/// under its own heading, and a reply with code but no schedule text keeps
/// it with the listing appended. Only a reply with none of these (no code,
/// no invented lines, every run named or none) grounds exactly as v4.
pub(super) fn ground(reply: &str, outcomes: &[ToolOutcome]) -> Grounded {
    let Some(schedule) = canonical_schedule_output(outcomes) else {
        return Grounded {
            text: reply.to_owned(),
            block: None,
        };
    };
    let full = |text: String| Grounded {
        text,
        block: Some(Block::full(schedule)),
    };
    if strip(reply) == strip(schedule) {
        return full(schedule.to_owned());
    }
    let ids: BTreeSet<String> = RUN_ID
        .captures_iter(schedule)
        .map(|found| found[1].to_lowercase())
        .collect();
    let lines = splitlines(reply);
    let code = fenced_lines(reply, &lines);
    let prose = |at: usize| at < lines.len() && !code[at];
    let known = |line: &str| mentions_id(line, &ids);

    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut guesses: Vec<(usize, usize)> = Vec::new();
    let mut named: BTreeSet<String> = BTreeSet::new();
    let mut index = 0;
    while index < lines.len() {
        if !prose(index) {
            index += 1;
            continue;
        }
        let line = lines[index];
        let next = prose(index + 1).then(|| lines[index + 1]);
        // A record line with its facts line, or a `**day — boss**` line
        // whose facts line carries the id.
        let record = known(line)
            && SCHEDULE_RUN_LINE.is_match(line)
            && line.contains("**")
            && next.is_some_and(has_schedule_facts);
        let primary = PRIMARY_LINE.is_match(line)
            && next.is_some_and(|next| known(next) && has_schedule_facts(next));
        if record || primary {
            named.extend(named_ids(line, &ids));
            named.extend(named_ids(next.unwrap_or_default(), &ids));
            spans.push((index, index + 2));
            index += 2;
        } else if known(line) && has_schedule_facts(line) {
            named.extend(named_ids(line, &ids));
            spans.push((index, index + 1));
            index += 1;
        } else if invented(line, outcomes)
            && (has_schedule_facts(line) || next.is_some_and(has_schedule_facts))
        {
            // Its facts line, never another record line.
            let facts = next.is_some_and(|next| {
                has_schedule_facts(next) && !known(next) && !SCHEDULE_RUN_LINE.is_match(next)
            });
            let end = if facts { index + 2 } else { index + 1 };
            guesses.push((index, end));
            index = end;
        } else {
            index += 1;
        }
    }

    if !spans.is_empty() {
        let partial = named != ids;
        let block = if partial {
            Block::only(schedule, &named).filter(|block| !block.text.is_empty())
        } else {
            None
        };
        let partial = block.is_some();
        // `(start, end, real)`: the listing goes at the first real run.
        let mut tagged: Vec<(usize, usize, bool)> = spans
            .into_iter()
            .map(|(start, end)| (start, end, true))
            .chain(guesses.into_iter().map(|(start, end)| (start, end, false)))
            .collect();
        tagged.sort_unstable();
        let blank = |at: usize| strip(lines[at]).is_empty();
        let mut blocks: Vec<[usize; 2]> = Vec::new();
        let mut first_real = None;
        for (start, end, real) in tagged {
            match blocks.last_mut() {
                Some(block) if (block[1]..start).all(blank) => block[1] = end,
                _ => blocks.push([start, end]),
            }
            if real && first_real.is_none() {
                first_real = Some(blocks.len() - 1);
            }
        }
        for block in &mut blocks {
            let mut heading = block[0];
            while heading > 0 && blank(heading - 1) {
                heading -= 1;
            }
            if !partial && heading > 0 && prose(heading - 1) && HEADING.is_match(lines[heading - 1])
            {
                block[0] = heading - 1;
            }
            let mut marker = block[1];
            while marker < lines.len() && blank(marker) {
                marker += 1;
            }
            if prose(marker) && (OMISSION.is_match(lines[marker]) || FOOTER.is_match(lines[marker]))
            {
                block[1] = marker + 1;
            }
        }
        let block = block.unwrap_or_else(|| Block::full(schedule));
        let mut rebuilt: Vec<&str> = Vec::new();
        let mut cursor = 0;
        for (number, [start, end]) in blocks.iter().copied().enumerate() {
            rebuilt.extend_from_slice(&lines[cursor..start]);
            if Some(number) == first_real {
                rebuilt.extend(splitlines(&block.text));
            }
            cursor = end;
        }
        rebuilt.extend_from_slice(&lines[cursor..]);
        return Grounded {
            text: strip(&rebuilt.join("\n")).to_owned(),
            block: Some(block),
        };
    }

    let hints: Vec<usize> = (0..lines.len())
        .filter(|&at| prose(at) && is_hint(lines[at]))
        .collect();
    if let (Some(&first), Some(&last)) = (hints.first(), hints.last()) {
        let before = lines[..first].join("\n");
        let after = lines[last + 1..].join("\n");
        // Code between the hints is the member's content, not a retelling.
        let mut kept: Vec<String> = Vec::new();
        let mut at = first;
        while at <= last {
            if code[at] {
                let start = at;
                while at <= last && code[at] {
                    at += 1;
                }
                kept.push(lines[start..at].join("\n"));
            } else {
                at += 1;
            }
        }
        let mut parts: Vec<&str> = vec![strip(&before), strip(schedule)];
        parts.extend(kept.iter().map(String::as_str));
        parts.push(strip(&after));
        let parts: Vec<&str> = parts.into_iter().filter(|part| !part.is_empty()).collect();
        return full(parts.join("\n\n"));
    }
    if code.contains(&true) {
        return full(format!("{}\n\n{schedule}", strip(reply)));
    }
    full(schedule.to_owned())
}
