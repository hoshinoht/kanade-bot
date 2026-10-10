//! Keep listed schedule facts in the tool's canonical rendering (v4
//! `_ground_schedule_reply`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use super::cite::voiced;
use super::dated::{Listed, named_by_time, records};
use super::fence::fenced;
use super::listing::Block;
use super::personal::{split, strip_context_copies};
use super::tidy::SCHEDULE_RUN_LINE;
use super::{pattern, pattern_i, splitlines};
use crate::chat::tools::{ToolName, ToolOutcome};
use crate::domain::pytext::strip;

static RECORD_ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"`?\[[0-9a-fA-F]{8}\]`?"));
static PRIMARY_LINE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^\s*(?:[-*]\s+)?\*\*.+ — .+\*\*$"));
static RUN_ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"\[([0-9a-fA-F]{8})\]"));
/// A run id as the model cites one: bracketed or backticked.
static CITED_ID: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"`\[?([0-9a-fA-F]{8})\]?`|\[([0-9a-fA-F]{8})\]"));
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\b(?:runs|boss week|schedule(?:d)?|all channels|this channel)\b"));
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

/// What grounding made of a reply, and the schedule text it put in.
pub(super) struct Grounded {
    pub text: String,
    pub block: Option<Block>,
}

/// Keep the model's wording with the canonical records of the runs it
/// names, replacing only retold records and lines with unlisted facts.
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

/// Whether any successful tool output carries `rid` (lowercase).
fn returned(rid: &str, outcomes: &[ToolOutcome]) -> bool {
    outcomes
        .iter()
        .any(|outcome| outcome.ok && outcome.output.to_lowercase().contains(rid))
}

/// A record-shaped line whose bracketed id no tool output carries.
fn invented(line: &str, outcomes: &[ToolOutcome]) -> bool {
    SCHEDULE_RUN_LINE.is_match(line)
        && RUN_ID
            .captures(line)
            .is_some_and(|found| !returned(&found[1].to_lowercase(), outcomes))
}

/// A line citing an id (bracketed or backticked) no tool output carries.
fn cites_unlisted(line: &str, outcomes: &[ToolOutcome]) -> bool {
    CITED_ID.captures_iter(line).any(|found| {
        let rid = found.get(1).or(found.get(2)).map_or("", |m| m.as_str());
        !returned(&rid.to_lowercase(), outcomes)
    })
}

/// v4 `_ground_schedule_reply` with the `D-GROUND-FILTERED` differences
/// (user decision 2026-10-03, "keep wording, card under"): the model's
/// wording stays and each listed run it names (by id in any form, or by a
/// time that, narrowed by a stated date or weekday, picks out one run) gets
/// its record once, after the paragraph naming it. A line is replaced only
/// when it retells a record (`[id] **Boss**` with its facts), cites an id
/// no tool returned, or states a time or date no listed run has; it becomes
/// the records of the runs it names, or the full listing when it names
/// none. Invented record lines go, fenced code is never read or replaced,
/// and a reply naming no run keeps its wording with the full listing after
/// it. Wrong counts, statuses, bosses or names in a kept line are not
/// checked (accepted gap). `D-VOICED-CARD`: a cited id reads as the run's
/// label, and records under a kept line come without the listing's
/// heading. `D-PERSONAL-CONTEXT`: context lines never reach the reply.
pub(super) fn ground(reply: &str, outcomes: &[ToolOutcome]) -> Grounded {
    let reply = strip_context_copies(reply, outcomes);
    let reply = reply.as_str();
    let Some(raw) = canonical_schedule_output(outcomes) else {
        return Grounded {
            text: reply.to_owned(),
            block: None,
        };
    };
    let schedule = split(raw);
    let schedule = schedule.as_str();
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
    let dated = records(schedule);
    let labels: BTreeMap<String, String> = dated
        .iter()
        .map(|run| (run.id.clone(), run.label.clone()))
        .collect();
    let listed = Listed::of(schedule);
    let lines = splitlines(reply);
    let code = fenced_lines(reply, &lines);
    let prose = |at: usize| at < lines.len() && !code[at];
    let known = |line: &str| mentions_id(line, &ids);

    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut guesses: Vec<(usize, usize)> = Vec::new();
    let mut named: BTreeSet<String> = BTreeSet::new();
    // A replaced line naming no run: the full listing goes in.
    let mut unnamed = false;
    let mut sentences: Vec<usize> = Vec::new();
    // Kept lines citing ids, as members see them.
    let mut cited: BTreeMap<usize, String> = BTreeMap::new();
    // The runs whose records go at each kept line's paragraph end.
    let mut introduces: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    // A kept line: an empty span at its paragraph's end takes the records.
    let after_paragraph = |at: usize| {
        let mut end = at + 1;
        while prose(end) && !strip(lines[end]).is_empty() {
            end += 1;
        }
        (end, end)
    };
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
            let runs: BTreeSet<String> = named_ids(line, &ids)
                .chain(named_by_time(line, &dated))
                .collect();
            // A one-line record retelling: `[id] **Boss** · facts`, or a
            // list line like `Boss - 21:30 - run ID 'id'`.
            let retold = known(line)
                && has_schedule_facts(line)
                && ((SCHEDULE_RUN_LINE.is_match(line) && line.contains("**"))
                    || RUN_ID_WORD.is_match(line));
            if retold || listed.contradicted(line) || cites_unlisted(line, outcomes) {
                unnamed |= runs.is_empty();
                named.extend(runs);
                spans.push((index, index + 1));
            } else if !runs.is_empty() {
                let span = after_paragraph(index);
                introduces.entry(span.0).or_default().extend(runs.clone());
                named.extend(runs);
                spans.push(span);
                sentences.push(index);
                cited.insert(index, voiced(line, &labels));
            }
            index += 1;
        }
    }

    if spans.is_empty() && guesses.is_empty() {
        // Nothing named or replaced: the wording, then the listing.
        let kept = strip(reply);
        if kept.is_empty() {
            return full(schedule.to_owned());
        }
        return full(format!("{kept}\n\n{schedule}"));
    }
    // Only kept lines (no replaced ones): each paragraph gets the records
    // of the runs it introduces.
    let only_sentences = spans.iter().all(|(start, end)| start == end);
    // `(start, end, real)`: the listing goes at the first real run.
    let mut tagged: Vec<(usize, usize, bool)> = spans
        .into_iter()
        .map(|(start, end)| (start, end, true))
        .chain(guesses.into_iter().map(|(start, end)| (start, end, false)))
        .collect();
    tagged.sort_unstable();
    let blank = |at: usize| strip(lines[at]).is_empty();
    // `inserted` blocks start after a kept line's paragraph.
    let mut blocks: Vec<[usize; 2]> = Vec::new();
    let mut inserted: Vec<bool> = Vec::new();
    let mut first_real = None;
    for (start, end, real) in tagged {
        match blocks.last_mut() {
            Some(block) if (block[1]..start).all(blank) => block[1] = block[1].max(end),
            _ => {
                blocks.push([start, end]);
                inserted.push(start == end);
            }
        }
        if real && first_real.is_none() {
            first_real = Some(blocks.len() - 1);
        }
    }
    // Only invented lines: the listing takes the first one's place.
    let first_real = first_real.unwrap_or(0);
    // Records placed under a kept line: the line introduces them, so no
    // listing heading repeats it (`D-VOICED-CARD`).
    let introduced = inserted[first_real];
    let block = if !unnamed && (named != ids || introduced) {
        Block::only(schedule, &named).filter(|block| !block.text.is_empty())
    } else {
        None
    };
    let partial = block.is_some();
    for (block, &inserted) in blocks.iter_mut().zip(&inserted) {
        let mut heading = block[0];
        while heading > 0 && blank(heading - 1) {
            heading -= 1;
        }
        if !partial
            && !inserted
            && heading > 0
            && !sentences.contains(&(heading - 1))
            && prose(heading - 1)
            && HEADING.is_match(lines[heading - 1])
        {
            block[0] = heading - 1;
        }
        let mut marker = block[1];
        while marker < lines.len() && blank(marker) {
            marker += 1;
        }
        if prose(marker) && (OMISSION.is_match(lines[marker]) || FOOTER.is_match(lines[marker])) {
            block[1] = marker + 1;
        }
    }
    let block = block.unwrap_or_else(|| Block::full(schedule));
    let mut pieces: Vec<Option<String>> = vec![None; blocks.len()];
    pieces[first_real] = Some(block.text.clone());
    if partial && introduced && only_sentences {
        let mut placed: BTreeSet<String> = BTreeSet::new();
        for (piece, [start, end]) in pieces.iter_mut().zip(&blocks) {
            let runs: BTreeSet<String> = introduces
                .range(*start..=*end)
                .flat_map(|(_, runs)| runs.iter().cloned())
                .filter(|run| placed.insert(run.clone()))
                .collect();
            *piece = Block::only(schedule, &runs)
                .map(|block| block.text)
                .filter(|text| !text.is_empty());
        }
    }
    let shown: Vec<&str> = (0..lines.len())
        .map(|at| cited.get(&at).map_or(lines[at], String::as_str))
        .collect();
    let mut rebuilt: Vec<&str> = Vec::new();
    let mut cursor = 0;
    for (number, [start, end]) in blocks.iter().copied().enumerate() {
        rebuilt.extend(&shown[cursor..start]);
        if let Some(piece) = &pieces[number] {
            // Set off from the line above and whatever follows.
            if inserted[number] {
                rebuilt.push("");
            }
            rebuilt.extend(splitlines(piece));
            if inserted[number] && end < lines.len() && !blank(end) {
                rebuilt.push("");
            }
        }
        cursor = end;
    }
    rebuilt.extend(&shown[cursor..]);
    Grounded {
        text: strip(&rebuilt.join("\n")).to_owned(),
        block: Some(block),
    }
}
