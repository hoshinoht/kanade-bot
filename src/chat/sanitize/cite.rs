//! `D-VOICED-CARD`: a line citing listed runs by id (`[id]`, `` `id` ``,
//! bare or `` `[id]` ``) is kept as voicing, each id read as the run's bold
//! boss label, only when it states no schedule fact once its ids, the
//! "your next one" glue and the cited runs' exact context phrases are
//! blanked out (one closed rule, user decision 2026-10-03; tuned from live
//! traces). Anything else is replaced by the cards.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::dated::{Dated, blank_context};
use super::personal::Contexts;
use super::{is_word, pattern, pattern_i};

static TOKEN: LazyLock<Regex> = LazyLock::new(|| pattern(r"`?\[?([0-9a-fA-F]{8})\]?`?"));
/// The cited run itself: "your next one" restates the card, not a new fact.
static GLUE: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\byour\s+next\s+(?:one|run|boss\s+run)\b"));
/// Words that state a schedule fact: weekdays, months (not `may`), relative
/// time and comparison, answers and statuses, channels and member mentions.
/// Digits are checked separately.
static FACT: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(concat!(
        r"\b(?:mon|tues?|wed(?:nes)?|thu(?:rs?)?|fri|sat(?:ur)?|sun)(?:days?)?\b",
        r"|\b(?:jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|june?|july?|aug(?:ust)?|sep(?:t|tember)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?)\b",
        r"|\b(?:today|tonight|tonite|tomorrow|tmrw?|tmw|yesterday|weeks?|weekends?|weekly|days?|daily|hours?|hrs?|minutes?|mins?|morning|afternoon|evening|night|noon|midnight|same|too|also|before|after|earlier|later|sooner|soon|then|next|last|first|second|third|again|until|till|since|ago|early|late|now|already)\b",
        r"|\b(?:yes|said|says|answer(?:s|ed)?|rsvp\w*|repl(?:y|ies|ied)|respond(?:s|ed)?|declined?|accepted?|maybe|skip(?:s|ped|ping)?|signed\s+up|planned|confirmed|unconfirmed|at[\s_-]+risk|otot|own[\s-]+time|done|cancel(?:l?ed)?|postponed?|rescheduled?|moved|delayed|finished|completed?|tentative|pending)\b",
        r"|<[#@][!&]?\d+>|(?:^|[^\w<&])[#@][a-z_][\w-]*",
    ))
});

/// One cited id: its byte span in the line and its run.
struct Cited<'a> {
    start: usize,
    end: usize,
    run: &'a Dated,
}

/// The listed runs a line cites, in order; `None` when an id names a listed
/// run whose record cannot be checked.
fn cited<'a>(line: &str, ids: &BTreeSet<String>, records: &'a [Dated]) -> Option<Vec<Cited<'a>>> {
    let mut found = Vec::new();
    for token in TOKEN.captures_iter(line) {
        let (whole, id) = (token.get(0)?, token[1].to_lowercase());
        let alone = !line[..whole.start()]
            .chars()
            .next_back()
            .is_some_and(is_word)
            && !line[whole.end()..].chars().next().is_some_and(is_word);
        if !alone || !ids.contains(&id) {
            continue;
        }
        found.push(Cited {
            start: whole.start(),
            end: whole.end(),
            run: records.iter().find(|record| record.id == id)?,
        });
    }
    Some(found)
}

fn starts_with_label(text: &str, label: &str) -> bool {
    text.trim_start_matches('*')
        .to_lowercase()
        .starts_with(&label.to_lowercase())
}

fn ends_with_label(text: &str, label: &str) -> bool {
    text.trim_end_matches('*')
        .to_lowercase()
        .ends_with(&label.to_lowercase())
}

/// The line with one id swapped for `**label**`, or dropped where the label
/// already stands next to it or the line names it beside a parenthesised id.
fn swap(text: &mut String, start: usize, end: usize, label: &str) {
    let wrapped = text[..start].ends_with('(') && text[end..].starts_with(')');
    let (start, end) = if wrapped {
        (start - 1, end + 1)
    } else {
        (start, end)
    };
    let separator = |c: char| c.is_whitespace() || "—–-:,".contains(c);
    let before = text[..start].trim_end_matches(separator);
    let after = text[end..].trim_start_matches(separator);
    if !label.is_empty() && ends_with_label(before, label) {
        text.replace_range(before.len()..end, "");
    } else if !label.is_empty() && starts_with_label(after, label) {
        let resume = text.len() - after.len();
        text.replace_range(start..resume, "");
    } else if wrapped && !label.is_empty() && text.to_lowercase().contains(&label.to_lowercase()) {
        let trimmed = text[..start].trim_end().len();
        text.replace_range(trimmed..end, "");
    } else {
        text.replace_range(start..end, &format!("**{label}**"));
    }
}

/// Whether `text` states a schedule fact under the closed rule.
pub(super) fn states_fact(text: &str) -> bool {
    text.chars().any(char::is_numeric) || FACT.is_match(text)
}

/// The member-facing line, when it states no fact beyond its cited ids and
/// those runs' exact context phrases; `None` falls back to the cards.
pub(super) fn voiced(
    line: &str,
    ids: &BTreeSet<String>,
    records: &[Dated],
    contexts: &Contexts,
) -> Option<String> {
    let cited = cited(line, ids, records)?;
    if cited.is_empty() {
        return None;
    }
    let mut rest = line.to_owned();
    for cite in &cited {
        rest.replace_range(cite.start..cite.end, &" ".repeat(cite.end - cite.start));
    }
    let phrases: Vec<String> = cited
        .iter()
        .flat_map(|cite| contexts.get(&cite.run.id).into_iter().flatten().cloned())
        .collect();
    let (_, rest) = blank_context(&rest, &phrases);
    if states_fact(&GLUE.replace_all(&rest, " ")) {
        return None;
    }
    let mut text = line.to_owned();
    for cite in cited.iter().rev() {
        swap(&mut text, cite.start, cite.end, &cite.run.label);
    }
    Some(text)
}
