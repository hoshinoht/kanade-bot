//! Untrusted run and weekly-timing descriptions → one row, never a guess
//! (v4 `tools/resolution.py`).

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, Utc, Weekday};
use regex::Regex;

use super::ToolWorld;
use super::format::{fixed_line, local, run_line};
use crate::chat::tools::{MAX_RUNS, ToolError, ToolResult};
use crate::domain::ids::resolve_id;
use crate::domain::pytext::strip;
use crate::domain::schedule::{FixedRun, Run, RunStatus, utc_instant};
use crate::domain::weeks::materialised_week_starts;
use crate::extract::resolve::WEEKDAY_ALIASES;

/// Day words `get_run` and `get_schedule` read besides weekdays.
pub(super) const RELATIVE_DAYS: [(&str, i64); 5] = [
    ("today", 0),
    ("tonight", 0),
    ("tomorrow", 1),
    ("tmr", 1),
    ("tmrw", 1),
];

/// `word` written in `query` as a whole word.
pub(super) fn says(query: &str, word: &str) -> bool {
    Regex::new(&format!(r"\b{}\b", regex::escape(word)))
        .expect("pattern")
        .is_match(query)
}

fn days_forward(from: Weekday, to: Weekday) -> i64 {
    i64::from((7 + to.num_days_from_monday() - from.num_days_from_monday()) % 7)
}

fn referenced_dates(world: &ToolWorld<'_>, query: &str, now: DateTime<Utc>) -> BTreeSet<NaiveDate> {
    let today = local(&now, world.zone).date();
    let mut dates: BTreeSet<NaiveDate> = WEEKDAY_ALIASES
        .iter()
        .filter(|(word, _)| says(query, word))
        .map(|&(_, weekday)| today + TimeDelta::days(days_forward(today.weekday(), weekday)))
        .collect();
    dates.extend(
        RELATIVE_DAYS
            .iter()
            .filter(|(word, _)| says(query, word))
            .map(|&(_, offset)| today + TimeDelta::days(offset)),
    );
    dates
}

fn names_a_day(query: &str) -> bool {
    WEEKDAY_ALIASES.iter().any(|(word, _)| says(query, word))
        || RELATIVE_DAYS.iter().any(|(word, _)| says(query, word))
}

/// Boss keys a list of canonical tokens names, through the alias table.
fn boss_shorts(world: &ToolWorld<'_>, tokens: &[String]) -> BTreeSet<String> {
    tokens
        .iter()
        .flat_map(|token| world.catalog.names_in(token))
        .collect()
}

/// Run records under `lead`, the way an ambiguous description lists them.
pub fn listing(world: &ToolWorld<'_>, runs: &[&Run], lead: &str, now: DateTime<Utc>) -> String {
    let lines: Vec<String> = runs
        .iter()
        .take(MAX_RUNS)
        .map(|run| run_line(world, run, false, now))
        .collect();
    format!("{lead}\n\n{}", lines.join("\n\n"))
}

fn no_run(text: &str) -> ToolError {
    ToolError(format!(
        "No run matches `{text}`. Check what is scheduled, then ask them which one they mean. Do not guess."
    ))
}

fn ambiguous(world: &ToolWorld<'_>, text: &str, runs: &[&Run], now: DateTime<Utc>) -> ToolError {
    ToolError(format!(
        "`{text}` matches more than one run. {}",
        listing(world, runs, "Ask which one:", now)
    ))
}

/// The runs a description can still change: neither cancelled nor done, in
/// the materialised boss weeks.
fn open_runs<'w>(world: &'w ToolWorld<'_>, now: DateTime<Utc>) -> ToolResult<Vec<&'w Run>> {
    let weeks = materialised_week_starts(world.zone, world.reset_weekday, world.reset_time, &now)
        .map_err(|error| ToolError(error.to_string()))?;
    let mut open: Vec<&Run> = Vec::new();
    for start in &weeks {
        let start = utc_instant(start).map_err(|error| ToolError(error.to_string()))?;
        open.extend(world.snapshot.runs.iter().filter(|run| {
            run.week_start == start && !matches!(run.status, RunStatus::Cancelled | RunStatus::Done)
        }));
    }
    Ok(open)
}

/// A run from a short id or an unambiguous boss/day description.
pub fn resolve_run<'w>(
    world: &'w ToolWorld<'_>,
    query: &str,
    now: DateTime<Utc>,
) -> ToolResult<&'w Run> {
    let text = strip(query);
    if text.is_empty() {
        return Err(ToolError::new(
            "Ask them which run they mean -- a boss and a day, like 'hstar wednesday'.",
        ));
    }
    let runs = &world.snapshot.runs;
    if let Ok(id) = resolve_id(text, runs.iter().map(|run| run.id.as_str())) {
        return Ok(runs.iter().find(|run| run.id == id).expect("resolved id"));
    }
    let low = text.to_lowercase();
    let dates = referenced_dates(world, &low, now);
    if dates.len() > 1 {
        return Err(ToolError(format!(
            "`{text}` names more than one day. Ask them which one they mean; do not guess."
        )));
    }
    let candidates = open_runs(world, now)?;
    let named: BTreeSet<String> = world.catalog.names_in(&low).into_iter().collect();
    let by_boss: Vec<&Run> = candidates
        .iter()
        .copied()
        .filter(|run| !named.is_empty() && !boss_shorts(world, &run.bosses).is_disjoint(&named))
        .collect();
    let named_day = !dates.is_empty();
    if by_boss.is_empty() && !named_day {
        return Err(no_run(text));
    }
    let mut matches = if by_boss.is_empty() {
        candidates.clone()
    } else {
        by_boss.clone()
    };
    if named_day {
        let narrowed: Vec<&Run> = matches
            .iter()
            .copied()
            .filter(|run| dates.contains(&local(&run.datetime, world.zone).date()))
            .collect();
        if !narrowed.is_empty() {
            matches = narrowed;
        } else if !by_boss.is_empty() {
            let same_weekday: Vec<&Run> = by_boss
                .iter()
                .copied()
                .filter(|run| {
                    let weekday = local(&run.datetime, world.zone).weekday();
                    WEEKDAY_ALIASES
                        .iter()
                        .any(|&(word, day)| day == weekday && says(&low, word))
                })
                .collect();
            if let [only] = same_weekday.as_slice() {
                return Ok(only);
            }
            return Err(ToolError(format!(
                "No run matches `{text}`. {}",
                listing(world, &by_boss, "That boss is on", now)
            )));
        }
    }
    match matches.as_slice() {
        [] => Err(no_run(text)),
        [only] => Ok(only),
        many => Err(ambiguous(world, text, many, now)),
    }
}

/// The asker's own words in the conversation the model was given, never the
/// model's: the question, and when it follows a bot reply directly, the
/// asker's message before that reply. Empty when no member asked (a
/// rejection follow-up's prompt, a lone tool call): nothing to check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Heard<'a> {
    pub question: &'a str,
    pub earlier: Option<&'a str>,
}

/// `words` carry `run`'s id or a unique prefix of it (`#a1b2c3d4`, pasted ids).
fn names_id(runs: &[Run], words: &str, run: &Run) -> bool {
    words
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '#' || ch == '-'))
        .any(|token| {
            resolve_id(token, runs.iter().map(|run| run.id.as_str())).is_ok_and(|id| id == run.id)
        })
}

/// What a message's words describe, judged against the run a tool resolved.
struct Described<'w> {
    /// The open runs the words fit: those with a boss the words name that
    /// `run` has (every named boss when it has none of them, so "hstar and
    /// kalos" singles out each), on a day the words name.
    fits: Vec<&'w Run>,
    /// The resolved run fits the words too.
    fits_run: bool,
}

/// `None` when the words name neither a boss nor a day. A day named only as
/// where a move goes (`to`'s date, "move the hstar to friday") does not
/// describe the run, unless the run is on it.
fn described<'w>(
    world: &ToolWorld<'_>,
    open: &[&'w Run],
    words: &str,
    run: &Run,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<Described<'w>> {
    let low = words.to_lowercase();
    let named: BTreeSet<String> = world.catalog.names_in(&low).into_iter().collect();
    let mut dates = referenced_dates(world, &low, now);
    if named.is_empty() && dates.is_empty() {
        return None;
    }
    let day = |candidate: &Run| local(&candidate.datetime, world.zone).date();
    if !dates.contains(&day(run))
        && let Some(to) = to
    {
        dates.remove(&local(&to, world.zone).date());
    }
    let own: BTreeSet<String> = boss_shorts(world, &run.bosses)
        .intersection(&named)
        .cloned()
        .collect();
    let bosses = if own.is_empty() { &named } else { &own };
    let fit = |candidate: &Run| {
        (bosses.is_empty() || !boss_shorts(world, &candidate.bosses).is_disjoint(bosses))
            && (dates.is_empty() || dates.contains(&day(candidate)))
    };
    Some(Described {
        fits: open
            .iter()
            .copied()
            .filter(|candidate| fit(candidate))
            .collect(),
        fits_run: (named.is_empty() || !own.is_empty()) && fit(run),
    })
}

/// `D-GUESSED-RUN`: a run a write tool resolved for the asker, checked
/// against the asker's own words ([`ToolWorld::heard`]) unless they carry
/// its id. Words that name a boss or day the run does not have refuse it
/// with the runs they do fit; words that fit it and other open runs `among`
/// allows refuse it with the ambiguity listing, unless a follow-up's earlier
/// message narrows them to it alone. Words naming no boss or day, or only
/// where a move goes (`to`), leave the model's choice alone.
pub fn require_heard(
    world: &ToolWorld<'_>,
    run: &Run,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    among: impl Fn(&Run) -> bool,
) -> ToolResult<()> {
    let heard = world.heard;
    let runs = &world.snapshot.runs;
    if heard.question.is_empty()
        || std::iter::once(heard.question)
            .chain(heard.earlier)
            .any(|words| names_id(runs, words, run))
    {
        return Ok(());
    }
    let open: Vec<&Run> = open_runs(world, now)?
        .into_iter()
        .filter(|candidate| among(candidate))
        .collect();
    let Some(question) = described(world, &open, heard.question, run, to, now) else {
        return Ok(());
    };
    let text = strip(heard.question);
    let mut fits = question.fits;
    if !question.fits_run {
        // The model picked a run the asker's words do not describe.
        return Err(match fits.as_slice() {
            [] => no_run(text),
            // One run fits their words and it is another: show both.
            [other] => ambiguous(world, text, &[other, run], now),
            many => ambiguous(world, text, many, now),
        });
    }
    if !fits.iter().any(|candidate| candidate.id == run.id) {
        fits.push(run);
    }
    if fits.len() <= 1 {
        return Ok(());
    }
    let narrowed = heard
        .earlier
        .and_then(|words| described(world, &open, words, run, to, now))
        .map(|before| {
            fits.iter()
                .copied()
                .filter(|candidate| {
                    if candidate.id == run.id {
                        before.fits_run
                    } else {
                        before.fits.iter().any(|other| other.id == candidate.id)
                    }
                })
                .collect::<Vec<_>>()
        });
    if let Some([only]) = narrowed.as_deref()
        && only.id == run.id
    {
        return Ok(());
    }
    Err(ambiguous(world, text, &fits, now))
}

fn no_weekly_for(world: &ToolWorld<'_>, text: &str) -> ToolResult<()> {
    let named = world.catalog.names_in(text);
    if named.is_empty() {
        return Ok(());
    }
    let scheduled: BTreeSet<String> = world
        .snapshot
        .fixed_runs
        .iter()
        .flat_map(|fixed| boss_shorts(world, &fixed.bosses))
        .collect();
    let missing: Vec<&String> = named
        .iter()
        .filter(|short| !scheduled.contains(*short))
        .collect();
    if missing.len() != named.len() {
        return Ok(());
    }
    let label: Vec<&str> = missing
        .iter()
        .filter_map(|short| world.catalog.boss(short))
        .map(|boss| boss.full())
        .collect();
    Err(ToolError(format!(
        "No weekly timing for {} exists, so there is nothing to change. If they are asking for an existing one-off run to happen every week, that is propose_add with weekly = true -- the scheduler folds this week's run into the new weekly instead of leaving a duplicate beside it. If they meant a different boss's weekly, ask them which one; do not offer them somebody else's.",
        label.join(", ")
    )))
}

/// A weekly timing from a short id or an unambiguous boss/day description.
pub fn resolve_fixed<'w>(world: &'w ToolWorld<'_>, query: &str) -> ToolResult<&'w FixedRun> {
    let text = strip(query);
    if text.is_empty() {
        return Err(ToolError::new(
            "Ask them which weekly timing they mean -- a boss, and a day if needed.",
        ));
    }
    let all = &world.snapshot.fixed_runs;
    if let Ok(id) = resolve_id(text, all.iter().map(|fixed| fixed.id.as_str())) {
        return Ok(all
            .iter()
            .find(|fixed| fixed.id == id)
            .expect("resolved id"));
    }
    let low = text.to_lowercase();
    let named: BTreeSet<String> = world.catalog.names_in(&low).into_iter().collect();
    let by_boss: Vec<&FixedRun> = all
        .iter()
        .filter(|fixed| !named.is_empty() && !boss_shorts(world, &fixed.bosses).is_disjoint(&named))
        .collect();
    if by_boss.is_empty() {
        no_weekly_for(world, text)?;
    }
    let named_day = names_a_day(&low);
    if by_boss.is_empty() && !named_day {
        return Err(ToolError(format!(
            "No weekly timing matches `{text}`. Ask them which boss's weekly run they mean."
        )));
    }
    let mut matches: Vec<&FixedRun> = if by_boss.is_empty() {
        all.iter().collect()
    } else {
        by_boss
    };
    if named_day {
        let narrowed: Vec<&FixedRun> = matches
            .iter()
            .copied()
            .filter(|fixed| {
                WEEKDAY_ALIASES
                    .iter()
                    .any(|&(word, day)| day == fixed.weekday && says(&low, word))
            })
            .collect();
        if !narrowed.is_empty() {
            matches = narrowed;
        }
    }
    match matches.as_slice() {
        [] => Err(ToolError(format!("No weekly timing matches `{text}`."))),
        [only] => Ok(only),
        many => {
            let listed: Vec<String> = many
                .iter()
                .take(MAX_RUNS)
                .map(|fixed| fixed_line(world, fixed))
                .collect();
            Err(ToolError(format!(
                "`{text}` matches more than one weekly timing. Ask which one they mean -- name the boss and the night each one is on, and do not pick one yourself. Their answer comes back as a normal message and you can try again then, with the short id in brackets if that is clearer:\n{}",
                listed.join("\n")
            )))
        }
    }
}
